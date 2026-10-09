//! The read-only web dashboard served by `nullrouter serve` on its own loopback port.
//!
//! Pages are server-rendered from the same read model the CLI prints (`nullrouter_server::views`);
//! nothing here changes state. Routes, headers and access rules:
//! `specs/009-dashboard/contracts/dashboard-http.md`.
//!
//! The dashboard runs beside the client listener and can't slow or break it: its own listener,
//! its own task, bounded page builds ([`guard`]), and a bind failure that is only recorded.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use nullrouter_engine::state::Engine;
use nullrouter_engine::status::DashboardStatus;
use nullrouter_registry::schema::DashboardSettings;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub mod access;
pub mod assets;
pub mod components;
pub mod frame;
pub mod guard;
pub mod headers;
pub mod landscape;
pub mod logos;
pub mod page;
pub mod pages;
pub mod time;
pub mod topology;

/// What every handler shares.
pub struct Shared {
    pub engine: Arc<Engine>,
    /// The version `nullrouter --version` prints, for the sidebar.
    pub version: String,
    pub guard: guard::Guard,
    /// The `Host` values this dashboard answers to; sign-in also checks `Origin` against it.
    pub rule: headers::HostRule,
    /// Wrong tokens since the last right one.
    pub access: access::Access,
}

/// A running dashboard (or the record of why there isn't one).
pub struct DashboardHandle {
    status: DashboardStatus,
    addr: Option<SocketAddr>,
    task: Option<JoinHandle<()>>,
}

impl DashboardHandle {
    /// What `server.status` reports.
    pub fn status(&self) -> &DashboardStatus {
        &self.status
    }

    /// The address the listener bound (the real port when `listen` asked for port 0); `None` when
    /// the bind failed.
    pub fn addr(&self) -> Option<SocketAddr> {
        self.addr
    }

    /// Resolves once the listener has stopped (immediately when it never started).
    pub async fn stopped(self) {
        if let Some(task) = self.task {
            let _ = task.await;
        }
    }
}

/// Binds `settings.listen` and serves until `shutdown` resolves; with `enabled = false` it binds
/// nothing and only records that. The bound state, or the reason the bind failed, goes to
/// `engine.status` for the operator socket. A bind failure is logged and
/// returned in the handle; it never stops `serve`.
pub async fn spawn(
    engine: Arc<Engine>,
    settings: &DashboardSettings,
    version: impl Into<String>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> DashboardHandle {
    let mut status =
        DashboardStatus { enabled: settings.enabled, listen: settings.listen.clone(), serving: false, error: None };
    if !settings.enabled {
        engine.status.set_dashboard(status.clone());
        return DashboardHandle { status, addr: None, task: None };
    }
    let listener = match TcpListener::bind(&settings.listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("the dashboard is not listening on {}: {e}", settings.listen);
            status.error = Some(format!("{}: {e}", settings.listen));
            engine.status.set_dashboard(status.clone());
            return DashboardHandle { status, addr: None, task: None };
        }
    };
    let addr = listener.local_addr().ok();
    // `listen` passed the loopback check by name (`localhost`); what it resolved to must be one.
    if let Some(a) = addr.filter(|a| !a.ip().is_loopback()) {
        let error = format!("{}: resolved to {a}, not a loopback address", settings.listen);
        tracing::warn!("the dashboard is not listening on {error}");
        status.error = Some(error);
        engine.status.set_dashboard(status.clone());
        return DashboardHandle { status, addr: None, task: None };
    }
    let port = addr.map_or(0, |a| a.port());
    let host = settings.listen.rsplit_once(':').map_or(settings.listen.as_str(), |(h, _)| h);
    let shared = Arc::new(Shared {
        engine: engine.clone(),
        version: version.into(),
        guard: guard::Guard::default(),
        rule: headers::HostRule::new(host, port),
        access: access::Access::default(),
    });
    let app = router(shared);
    status.serving = true;
    engine.status.set_dashboard(status.clone());
    tracing::info!("the dashboard is listening on {}", settings.listen);
    let task = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown).await {
            tracing::warn!("the dashboard stopped: {e}");
        }
    });
    DashboardHandle { status, addr, task: Some(task) }
}

/// Every route, behind the Host and method checks and the response headers. Sign-in and the
/// assets are open; every other path goes through the cookie check first.
pub fn router(shared: Arc<Shared>) -> Router {
    let rule = shared.rule.clone();
    Router::new()
        .route("/signin", get(pages::signin::get).post(pages::signin::post))
        .fallback(gated)
        .with_state(shared)
        .layer(axum::middleware::from_fn_with_state(rule, headers::enforce))
}

/// Any path but `/signin`: assets need no cookie; the rest redirect to sign in without one.
async fn gated(State(shared): State<Arc<Shared>>, req: Request) -> Response {
    let path = req.uri().path();
    if path.starts_with("/assets/") {
        return assets::serve(path);
    }
    match access::gate(&shared.engine, req.headers()) {
        access::Gate::NoToken => pages::signin::see_other("/signin"),
        access::Gate::SignedOut => {
            let wanted = req.uri().path_and_query().map_or(path, |p| p.as_str());
            pages::signin::see_other(&format!("/signin?next={}", access::encode_component(wanted)))
        }
        access::Gate::SignedIn if path == "/" => pages::signin::see_other("/endpoint"),
        access::Gate::SignedIn if path.starts_with("/logos/") => logos::serve(&shared.engine, path),
        access::Gate::SignedIn => match pages::Req::parse(path, req.uri().query().unwrap_or_default()) {
            Some(page_req) => page(shared, page_req).await,
            None => frame::render_error(None, &shared.version, StatusCode::NOT_FOUND, "No such page."),
        },
    }
}

/// One page, built under the guard (research R8): its views in one consistent state, then the
/// markup, both in the guarded task so a panic or a stall is this page's error only.
async fn page(shared: Arc<Shared>, req: pages::Req) -> Response {
    let (engine, version) = (shared.engine.clone(), shared.version.clone());
    let r = req.clone();
    let built = shared
        .guard
        .run(move || async move {
            let tz = time::local_zone();
            let logos = logos::Index::of(&engine);
            match read(&engine, &r).await {
                Ok(p) => frame::render(&r, &p, &version, &tz, &logos),
                // A window whose id doesn't exist: the page under it, and the CLI's message.
                Err(page::PageError::View(e)) if r.window.is_some() => {
                    let base = pages::Req { window: None, ..r.clone() };
                    match read(&engine, &base).await {
                        Ok(p) => frame::render_missing_window(&r, &p, &version, &tz, &logos, &e.message),
                        Err(e) => frame::render_error(
                            Some(&r),
                            &version,
                            StatusCode::INTERNAL_SERVER_ERROR,
                            &frame::build_failed(&e),
                        ),
                    }
                }
                Err(e) => {
                    frame::render_error(Some(&r), &version, StatusCode::INTERNAL_SERVER_ERROR, &frame::build_failed(&e))
                }
            }
        })
        .await;
    built.unwrap_or_else(|e| frame::render_error(Some(&req), &shared.version, e.status(), &e.text()))
}

/// The views `req` needs, or the CLI's "no record <id>" for a record id no record can have,
/// without reading the journal for it.
async fn read(engine: &Arc<Engine>, req: &pages::Req) -> Result<page::Page, page::PageError> {
    match req.impossible_record() {
        Some(id) => Err(page::PageError::View(nullrouter_server::views::ViewError::failed(format!("no record {id}")))),
        None => page::build(engine, &pages::wants(req)).await,
    }
}
