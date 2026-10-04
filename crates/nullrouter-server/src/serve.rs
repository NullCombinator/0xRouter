//! The server: one fallback handler that matches a style route, checks the access key and
//! dispatches. The route table follows the engine snapshot and is rebuilt after a reload.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{Request, State};
use axum::response::Response;
use axum::serve::ListenerExt;
use nullrouter_engine::attempt::Failure;
use nullrouter_engine::clock;
use nullrouter_engine::keys::AgentId;
use nullrouter_engine::records::{self, Outcome, RequestRecord};
use nullrouter_engine::state::{Engine, EngineState};
use nullrouter_registry::schema::{ModelType, RouteOp};
use nullrouter_wire::error_body;
use nullrouter_wire::primitives::{body, session};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::auth;
use crate::relay;
use crate::router::{Matched, RouteTable, pick};
use crate::{count, media, models, text};

pub struct App {
    pub engine: Arc<Engine>,
    routes: Mutex<(u64, Arc<RouteTable>)>,
}

impl App {
    /// Fails if a loaded style's routes don't build.
    pub fn new(engine: Arc<Engine>) -> Result<Arc<Self>, String> {
        let st = engine.snapshot();
        let table = Arc::new(RouteTable::build(st.registry.styles())?);
        Ok(Self::with_routes(engine, table))
    }

    /// Serves `table` until the next reload.
    pub fn with_routes(engine: Arc<Engine>, table: Arc<RouteTable>) -> Arc<Self> {
        let generation = engine.snapshot().generation;
        Arc::new(Self { engine, routes: Mutex::new((generation, table)) })
    }

    /// The route table for `st`. Built once per generation; a failed build keeps the old one.
    fn table(&self, st: &EngineState) -> Arc<RouteTable> {
        let mut cur = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        if cur.0 != st.generation {
            match RouteTable::build(st.registry.styles()) {
                Ok(t) => *cur = (st.generation, Arc::new(t)),
                Err(e) => tracing::error!("route table not rebuilt, keeping the previous one: {e}"),
            }
        }
        cur.1.clone()
    }
}

pub fn router(app: Arc<App>) -> axum::Router {
    axum::Router::new().fallback(dispatch).with_state(app)
}

pub(crate) fn style_error(m: &Matched<'_>, status: u16, message: &str, id: &str) -> Response {
    let body = error_body::body(&m.entry.style.codec, status, message, json!({ "record_id": id }));
    relay::json(status, &body, id)
}

/// An engine failure in the client's style: the attempt details, and `retry-after` when
/// every account is resting (research R11).
pub(crate) fn style_failure(m: &Matched<'_>, f: &Failure, id: &str) -> Response {
    let details = if f.tried.is_empty() { json!({ "record_id": id }) } else { error_body::details(id, &f.tried) };
    let body = error_body::body(&m.entry.style.codec, f.status, &f.message, details);
    let mut r = relay::json(f.status, &body, id);
    if let Some(secs) = f.retry_after {
        r.headers_mut().insert(axum::http::header::RETRY_AFTER, secs.into());
    }
    r
}

/// The largest request body read (base64 images and audio included).
pub const MAX_BODY: usize = 32 << 20;

async fn dispatch(State(app): State<Arc<App>>, req: Request) -> Response {
    let id = records::new_id();
    let started = Instant::now();
    let arrived = clock::now_rfc3339_millis();
    let st = app.engine.snapshot();
    let table = app.table(&st);
    let (parts, body) = req.into_parts();
    let path = parts.uri.path();
    let candidates = table.candidates(&parts.method, path, &parts.headers);
    let Some(first) = candidates.first() else {
        let msg = format!("0router: no route for {} {path}", parts.method);
        return relay::json(404, &error_body::openai(404, "invalid_request_error", &msg), &id);
    };
    let style = &first.entry.style;
    let mut record = RequestRecord::new(id.clone(), arrived, style.file.id.clone());
    record.op = Some(first.entry.route.op);
    record.model_type = Some(first.entry.route.model_type);

    let key = match auth::check(&st.keys, &style.file.access_key.carriers, &parts.headers, parts.uri.query()) {
        Ok(k) => k,
        Err(refusal) => {
            record.outcome = Outcome::Refused;
            app.engine.records.insert(record);
            return style_error(first, 401, refusal.message(), &id);
        }
    };
    record.agent = Some(AgentId::new(key.id.clone(), None));

    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => {
            record.outcome = Outcome::Failed;
            app.engine.records.insert(record);
            return style_error(first, 413, &format!("0router: the request body is over {} MiB", MAX_BODY >> 20), &id);
        }
    };
    let ctype = parts.headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default();
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else if ctype.starts_with("multipart/form-data") {
        match body::boundary(ctype)
            .ok_or_else(|| "no boundary".to_owned())
            .and_then(|b| body::parse_multipart(&bytes, b))
        {
            Ok(v) => v,
            Err(e) => {
                record.outcome = Outcome::Failed;
                app.engine.records.insert(record);
                return style_error(first, 400, &format!("0router: the multipart body doesn't parse: {e}"), &id);
            }
        }
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                record.outcome = Outcome::Failed;
                app.engine.records.insert(record);
                return style_error(first, 400, &format!("0router: the request body isn't JSON: {e}"), &id);
            }
        }
    };
    let Some(m) = pick(&candidates, &json) else {
        record.outcome = Outcome::Failed;
        app.engine.records.insert(record);
        return style_error(first, 404, &format!("0router: no route for {} {path} with this body", parts.method), &id);
    };
    let route = &m.entry.route;
    let header = |h: &str| parts.headers.get(h).and_then(|v| v.to_str().ok());
    let session = session::extract(&m.entry.style.file.session.carriers, header, &json);
    let agent = AgentId::new(key.id.clone(), session.as_deref());
    record.style.clone_from(&m.entry.style.file.id);
    record.op = Some(route.op);
    record.model_type = Some(route.model_type);
    record.agent = Some(agent.clone());

    match (route.op, route.model_type) {
        (RouteOp::Generate, ModelType::Text) => {
            app.engine.records.insert(record);
            let inc = text::Incoming { id, arrived: started, path, headers: parts.headers.clone(), body: json, agent };
            text::generate(&app.engine, st, m, inc).await
        }
        (RouteOp::Generate, _) | (RouteOp::JobSubmit, _) => {
            app.engine.records.insert(record);
            let inc = text::Incoming { id, arrived: started, path, headers: parts.headers.clone(), body: json, agent };
            media::generate(&app.engine, st, m, inc, route.op == RouteOp::JobSubmit).await
        }
        (RouteOp::JobGet, _) | (RouteOp::JobContent, _) => {
            // A poll or content fetch belongs to the submit's record.
            let inc = text::Incoming { id, arrived: started, path, headers: parts.headers.clone(), body: json, agent };
            if route.op == RouteOp::JobGet {
                media::job_get(&app.engine, st, m, inc).await
            } else {
                media::job_content(&app.engine, st, m, inc).await
            }
        }
        (RouteOp::CountTokens, _) => {
            app.engine.records.insert(record);
            let inc = text::Incoming { id, arrived: started, path, headers: parts.headers.clone(), body: json, agent };
            count::count(&app.engine, st, m, inc).await
        }
        (RouteOp::ListModels | RouteOp::GetModel, _) => {
            record.outcome = Outcome::Succeeded;
            app.engine.records.insert(record);
            match route.op {
                RouteOp::ListModels => models::list(&st, m, &id),
                _ => models::get(&st, m, m.capture("model").unwrap_or_default(), &id),
            }
        }
    }
}

/// Serves `app` on `listener` until `shutdown` resolves, then lets open requests finish.
pub async fn run(
    app: Arc<App>,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    // A crash left requests open and perhaps a torn last line: make the segments whole before
    // anything is served (spec 006, FR-037).
    let engine = app.engine.clone();
    match tokio::task::spawn_blocking(move || engine.recover_journal()).await {
        Ok(Ok(0)) => {}
        Ok(Ok(n)) => tracing::warn!("{n} requests were still open when 0router last stopped; recorded as interrupted"),
        Ok(Err(e)) => tracing::warn!("the record journal could not be checked: {e}"),
        Err(e) => tracing::warn!("the record journal check did not finish: {e}"),
    }
    // Stream frames are small: without TCP_NODELAY, Nagle holds the first one for the
    // client's delayed ACK (~40 ms on Linux).
    let listener = listener.tap_io(|tcp| {
        if let Err(e) = tcp.set_nodelay(true) {
            tracing::warn!("TCP_NODELAY not set: {e}");
        }
    });
    axum::serve(listener, router(app)).with_graceful_shutdown(shutdown).await
}

/// Resolves on SIGINT or SIGTERM.
pub async fn signal() {
    let int = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = int.await;
                return;
            }
        };
        tokio::select! {
            _ = int => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = int.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nullrouter_engine::keys::{self, Keys};
    use nullrouter_engine::records::Query;
    use nullrouter_registry::OperatorHome;

    #[tokio::test]
    async fn refuses_bad_keys_in_the_style_shape_and_404s_unknown_paths() {
        let dir = tempfile::tempdir().unwrap();
        let mut keys = Keys::default();
        let (good, _) = keys.issue("laptop", None).unwrap();
        nullrouter_engine::files::write_private(&dir.path().join(keys::FILE), &keys.to_toml()).unwrap();
        let (engine, _) = Engine::open(OperatorHome::new(dir.path())).unwrap();
        let engine = Arc::new(engine);
        let style = crate::router::tests::style_with_routes("mini", "routes = []");
        let app = App::with_routes(engine.clone(), Arc::new(RouteTable::build([&style]).unwrap()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(run(app, listener, async {
            let _ = stopped.await;
        }));
        let c = reqwest::Client::new();
        let url = format!("http://{addr}/mini/messages");

        let r = c.post(&url).body("{}").send().await.unwrap();
        assert_eq!(r.status(), 401);
        let id = r.headers()[relay::REQUEST_ID].to_str().unwrap().to_owned();
        let body: serde_json::Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["type"], "authentication_error");
        assert_eq!(body["nullrouter"]["record_id"], id.as_str());
        let rec = engine.records.get(&id).unwrap();
        assert_eq!(rec.outcome, Outcome::Refused);
        assert!(rec.agent.is_none());

        let r = c.post(&url).header("x-api-key", "0r-unknown").send().await.unwrap();
        assert_eq!(r.status(), 401);

        let r = c.post(&url).header("x-api-key", &good).send().await.unwrap();
        assert_eq!(r.status(), 400, "authenticated; the body names no model");

        let r = c.get(format!("http://{addr}/nope")).send().await.unwrap();
        assert_eq!(r.status(), 404);
        assert!(r.headers().contains_key(relay::REQUEST_ID));
        let body: serde_json::Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
        assert_eq!(body["error"]["type"], "invalid_request_error");

        assert_eq!(engine.records.query(&Query::default()).len(), 3, "no record for an unknown path");
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    }
}
