//! The operator socket (contracts/operator-cli.md § Operator socket protocol): NDJSON over
//! a Unix socket at `$NULLROUTER_HOME/run/operator.sock`, mode 0600 inside a 0700
//! directory. One request per line, one response per line.
//!
//! | Request | Response |
//! |---|---|
//! | `{"op":"reload"}` | `{"ok":true,"generation":N}` (plus `"notes":[…]` when unified models' limits differ), or the error with the old snapshot kept |
//! | `{"op":"records.list","provider"?,"unified_model"?,"account"?,"agent"?,"model"?,"reason"?,"since"?,"limit"?,"before"?}` | `{"ok":true,"records":[…]}`: the journal's records plus those still in flight, newest first, those with an id below `before` (which must name a record, else `{"ok":false,"error":"no record rq_…"}`) |
//! | `{"op":"records.get","id":"rq_…"}` | `{"ok":true,"record":{…}}` |
//! | `{"op":"keys.last_used"}` | `{"ok":true,"last_used":{"<key id>":"<RFC 3339>"\|null}}`: for every key in `keys.toml`, the arrival of its newest record, from the cached segment index and the requests still in flight; `null` for a key no record names |
//! | `{"op":"records.forget","account"?:"P/N","agent"?:KEY}` | `{"ok":true,"fingerprints":N}`: the agent's fingerprints (or the account's fingerprints and ledger entries) leave memory and `routing/warm.jsonl`, and the live ring; the CLI then rewrites the record segments |
//! | `{"op":"accounts.state"}` | `{"ok":true,"accounts":[…]}`: per account `kind`, `state`, `state_since`, `state_reason`, `expires_at`, cooldowns |
//! | `{"op":"routing.view","target"?}` | `{"ok":true,"amortization":{start,length},"journal":{…},"targets":[…],"warnings":[…]}`: per target and account the pace, share, deficit, priority, cache lifetime, quota source and each window's remaining amount, unit, reset and reserve |
//! | `{"op":"routing.health"}` | `{"ok":true,"journal":{kept,since,unkept_requests,held_lines,last_sync,last_sync_age_s}}` |
//! | `{"op":"server.status"}` | `{"ok":true,"client_listen":"…"\|null,"dashboard":{enabled,listen,serving,error}}`: the address `serve` bound for clients, and the dashboard listener's state |
//! | `{"op":"quota.list"}`, `{"op":"quota.poll"}`, `{"op":"quota.checkpoint"}` | see [`crate::quota`] |

use std::future::Future;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nullrouter_engine::journal::records;
use nullrouter_engine::records::Query;
use nullrouter_engine::state::Engine;
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

/// The socket's path under the operator home.
pub fn socket_path(home: &OperatorHome) -> PathBuf {
    home.path().join("run").join("operator.sock")
}

/// Binds the socket, replacing a stale one. Refuses when another server answers on it.
pub fn bind(home: &OperatorHome) -> std::io::Result<UnixListener> {
    let path = socket_path(home);
    let dir = path.parent().unwrap_or(home.path());
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("{}: another server is running on this home", path.display()),
            ));
        }
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Answers on `listener` until `shutdown` resolves, then removes the socket file.
pub async fn serve(engine: Arc<Engine>, listener: UnixListener, shutdown: impl Future<Output = ()>) {
    let path = listener.local_addr().ok().and_then(|a| a.as_pathname().map(Path::to_owned));
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            conn = listener.accept() => match conn {
                Ok((stream, _)) => { tokio::spawn(connection(engine.clone(), stream)); }
                Err(e) => tracing::warn!("operator socket: {e}"),
            },
        }
    }
    if let Some(p) = path {
        let _ = std::fs::remove_file(p);
    }
}

async fn connection(engine: Arc<Engine>, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(req) => handle(&engine, &req).await,
            Err(e) => json!({"ok": false, "error": format!("not JSON: {e}")}),
        };
        let mut out = answer.to_string();
        out.push('\n');
        if write.write_all(out.as_bytes()).await.is_err() {
            return;
        }
    }
}

/// The listeners `serve` bound (spec 009, R8).
fn server_status(engine: &Engine) -> Value {
    let d = engine.status.dashboard();
    json!({
        "ok": true,
        "client_listen": engine.status.client_listen(),
        "dashboard": { "enabled": d.enabled, "listen": d.listen, "serving": d.serving, "error": d.error },
    })
}

/// One request's answer.
pub async fn handle(engine: &Arc<Engine>, req: &Value) -> Value {
    let str_of = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_owned);
    match req.get("op").and_then(Value::as_str) {
        Some("reload") => match engine.reload().await {
            Ok(r) => {
                for a in &r.unused_accounts {
                    tracing::warn!("account {a} names a provider that isn't loaded");
                }
                let notes: Vec<String> =
                    engine.snapshot().registry.report().notes.iter().map(ToString::to_string).collect();
                for n in &notes {
                    tracing::info!("{n}");
                }
                let mut answer = json!({"ok": true, "generation": r.generation});
                if !notes.is_empty() {
                    answer["notes"] = json!(notes);
                }
                answer
            }
            Err(e) => {
                let error = engine.snapshot().redactor.redact(&e.to_string()).into_owned();
                tracing::error!("reload failed, the previous state stays: {error}");
                json!({"ok": false, "error": error})
            }
        },
        Some("records.list") => records_list(engine, req).await,
        Some("records.get") => {
            let Some(id) = str_of("id") else { return json!({"ok": false, "error": "the request names no id"}) };
            if let Some(r) = engine.records.get(&id) {
                return json!({"ok": true, "record": r});
            }
            let home = engine.home().path().to_owned();
            let found = tokio::task::spawn_blocking(move || nullrouter_engine::journal::records::get(&home, &id)).await;
            match found {
                Ok(Some(r)) => json!({"ok": true, "record": r}),
                _ => json!({"ok": false, "error": "no such record"}),
            }
        }
        Some("keys.last_used") => keys_last_used(engine).await,
        Some("records.forget") => {
            let st = engine.snapshot();
            let now = nullrouter_engine::clock::now();
            let (account, agent) = (str_of("account"), str_of("agent"));
            let fingerprints = match (&account, &agent) {
                (Some(a), None) => match a.split_once('/') {
                    Some((p, n)) => {
                        engine
                            .records
                            .forget(|r| records::Filter { account: Some(a.clone()), ..Default::default() }.matches(r));
                        nullrouter_engine::route::drop_account(engine, &st, p, n, now)
                    }
                    None => return json!({"ok": false, "error": "--account is provider/name"}),
                },
                (None, Some(k)) => {
                    engine
                        .records
                        .forget(|r| records::Filter { agent: Some(k.clone()), ..Default::default() }.matches(r));
                    nullrouter_engine::route::drop_agent(engine, &st, k, now)
                }
                _ => return json!({"ok": false, "error": "name an account or an agent, not both"}),
            };
            json!({"ok": true, "fingerprints": fingerprints})
        }
        Some("accounts.state") => {
            let st = engine.snapshot();
            let active = engine.cooldowns.active();
            let accounts: Vec<Value> = st
                .accounts
                .iter()
                .map(|a| {
                    let cooling: Vec<Value> = active
                        .iter()
                        .filter(|(p, n, _, _)| *p == a.provider && *n == a.name)
                        .map(|(_, _, m, left)| json!({"model": m, "remaining_ms": left.as_millis() as u64}))
                        .collect();
                    // Sign-in states (research R10); tokens never cross the socket.
                    let state = engine.tokens.state(a);
                    let time = nullrouter_engine::clock::rfc3339;
                    let expires_at = a
                        .is_signin()
                        .then(|| engine.tokens.get(&a.provider, &a.name))
                        .flatten()
                        .map(|v| time(v.entry.expires_at));
                    // `needs_sign_in` without tokens has no time.
                    let since = state.since().filter(|t| *t > std::time::UNIX_EPOCH).map(time);
                    json!({
                        "provider": a.provider,
                        "name": a.name,
                        "kind": if a.is_signin() { "signin" } else { "key" },
                        "disabled": a.disabled,
                        "state": state.name(),
                        "state_since": since,
                        "state_reason": state.reason(),
                        "expires_at": expires_at,
                        "level": engine.cooldowns.level(&a.provider, &a.name),
                        "cooling": cooling,
                    })
                })
                .collect();
            json!({"ok": true, "accounts": accounts})
        }
        Some("routing.view") => routing_view(engine, str_of("target").as_deref()),
        Some("routing.health") => json!({"ok": true, "journal": journal_health(engine)}),
        Some("server.status") => server_status(engine),
        Some("quota.list") => crate::quota::list(engine, req),
        Some("quota.poll") => crate::quota::poll_now(engine, req).await,
        Some("quota.checkpoint") => crate::quota::checkpoint(engine).await,
        Some(op) => json!({"ok": false, "error": format!("unknown op {op:?}")}),
        None => json!({"ok": false, "error": "the request names no op"}),
    }
}

/// `keys.last_used`: the journal read runs on the blocking pool, as `records.get`'s does.
async fn keys_last_used(engine: &Arc<Engine>) -> Value {
    use nullrouter_engine::keys::{self, Keys};

    let home = engine.home().path().to_owned();
    let read = tokio::task::spawn_blocking(move || {
        let list = Keys::load(&home.join(keys::FILE))?;
        let ids: Vec<String> = list.iter().map(|k| k.id.clone()).collect();
        Ok::<_, nullrouter_engine::files::FileError>((ids, records::last_arrived(&home)))
    })
    .await;
    let (ids, mut last) = match read {
        Ok(Ok(read)) => read,
        Ok(Err(e)) => return json!({"ok": false, "error": e.to_string()}),
        Err(e) => return json!({"ok": false, "error": format!("the read failed: {e}")}),
    };
    // A request in flight is a record too, and may not be in the journal yet.
    let parse = nullrouter_engine::clock::parse_rfc3339;
    for r in engine.records.query(&Query::default()) {
        let Some(agent) = &r.agent else { continue };
        let newer = match last.get(&agent.key) {
            Some(known) => parse(&r.arrived).zip(parse(known)).is_some_and(|(new, old)| new > old),
            None => parse(&r.arrived).is_some(),
        };
        if newer {
            last.insert(agent.key.clone(), r.arrived.clone());
        }
    }
    let shown: serde_json::Map<String, Value> =
        ids.into_iter().map(|id| (id.clone(), last.get(&id).map_or(Value::Null, |t| json!(t)))).collect();
    json!({"ok": true, "last_used": shown})
}

/// `records.list`: what the journal holds plus what is still in flight (the live ring is the
/// fresher copy of a record it has). Reading the segments happens off the executor.
async fn records_list(engine: &Arc<Engine>, req: &Value) -> Value {
    let str_of = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_owned);
    let limit = req.get("limit").and_then(Value::as_u64).map(|n| n as usize);
    let filter = records::Filter {
        provider: str_of("provider"),
        unified_model: str_of("unified_model"),
        account: str_of("account"),
        agent: str_of("agent"),
        model: str_of("model"),
        reason: str_of("reason"),
        since: str_of("since").and_then(|s| nullrouter_engine::clock::parse_rfc3339(&s)),
        limit,
        before: str_of("before"),
        test: req.get("test").and_then(Value::as_bool),
    };
    let home = engine.home().path().to_owned();
    if let Some(id) = &filter.before {
        let (h, wanted) = (home.clone(), id.clone());
        let named = tokio::task::spawn_blocking(move || records::cursor_exists(&h, &wanted)).await.unwrap_or(false);
        // A request still in flight is a record too, and only the server knows it.
        let live = engine.records.query(&Query::default()).iter().any(|r| &r.id == id);
        if !named && !live {
            return json!({"ok": false, "error": format!("no record {id}")});
        }
    }
    let read = filter.clone();
    let mut disk = tokio::task::spawn_blocking(move || records::read(&home, &read)).await.unwrap_or_default();
    let live: Vec<Value> = engine
        .records
        .query(&Query::default())
        .iter()
        .filter_map(|r| serde_json::to_value(r).ok())
        .filter(|r| filter.matches(r))
        .collect();
    let ids: std::collections::BTreeSet<String> =
        live.iter().filter_map(|r| r["id"].as_str().map(str::to_owned)).collect();
    disk.retain(|r| r["id"].as_str().is_none_or(|id| !ids.contains(id)));
    disk.extend(live);
    disk.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));
    disk.truncate(limit.unwrap_or(usize::MAX));
    json!({"ok": true, "records": disk})
}

/// The journal's health, as the routing view and `check` show it.
fn journal_health(engine: &Engine) -> Value {
    let h = engine.journal.health();
    let time = |t: std::time::SystemTime| nullrouter_engine::clock::rfc3339(t);
    json!({
        "kept": h.kept,
        "since": h.since.map(time),
        "unkept_requests": h.unkept_requests,
        "held_lines": h.held_lines,
        "last_sync": h.last_sync.map(time),
        "last_sync_age_s": h.last_sync.and_then(|t| t.elapsed().ok()).map(|d| d.as_secs_f64()),
    })
}

/// `routing.view`: every target's accounts as the next cold decision sees them.
fn routing_view(engine: &Engine, target: Option<&str>) -> Value {
    let st = engine.snapshot();
    let now = nullrouter_engine::clock::now();
    let targets = nullrouter_engine::route::view_all(engine, &st, target, now);
    let mut warnings: Vec<String> = targets.iter().flat_map(nullrouter_engine::routing::view::warnings).collect();
    warnings.dedup();
    let health = journal_health(engine);
    if health["kept"] == false {
        warnings.push(format!(
            "records not kept since {}: {} requests",
            health["since"].as_str().unwrap_or("?"),
            health["unkept_requests"]
        ));
    }
    let length = st.settings().routing.amortization;
    let default = nullrouter_engine::routing::AmortizationWindow {
        start: nullrouter_engine::routing::ledger::window_start(now, length),
        length,
    };
    json!({
        "ok": true,
        "now": nullrouter_engine::clock::rfc3339(now),
        "amortization": default,
        "journal": health,
        "targets": targets,
        "warnings": warnings,
    })
}

/// Why [`call`] got no answer.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("no server is running on {0}")]
    NoServer(String),
    #[error("operator socket: {0}")]
    Io(#[from] std::io::Error),
    #[error("operator socket: bad answer: {0}")]
    BadAnswer(String),
}

/// Sends one request to the running server and waits for its answer (blocking; for the
/// CLI).
pub fn call(home: &OperatorHome, req: &Value) -> Result<Value, CallError> {
    let path = socket_path(home);
    let mut stream =
        std::os::unix::net::UnixStream::connect(&path).map_err(|_| CallError::NoServer(path.display().to_string()))?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    let mut line = req.to_string();
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    serde_json::from_str(&answer).map_err(|e| CallError::BadAnswer(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answers_over_the_socket_and_replaces_a_stale_one() {
        let dir = tempfile::tempdir().unwrap();
        let home = OperatorHome::new(dir.path());
        let (engine, _) = Engine::open(home.clone()).unwrap();
        let engine = Arc::new(engine);
        engine.history.tally.attempt("p", "a", "m", None);
        // A stale file from a server that died.
        std::fs::create_dir_all(dir.path().join("run")).unwrap();
        std::fs::write(socket_path(&home), "").unwrap();
        let listener = bind(&home).unwrap();
        let mode = std::fs::metadata(socket_path(&home)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(bind(&home).is_err(), "a live socket is not taken over");
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(serve(engine.clone(), listener, async move {
            let _ = stopped.await;
        }));
        let h = home.clone();
        let answers = tokio::task::spawn_blocking(move || {
            [
                json!({"op": "reload"}),
                json!({"op": "records.get", "id": "rq_x"}),
                json!({"op": "nope"}),
                json!({"op": "quota.checkpoint"}),
                json!({"op": "records.list", "before": "rq_nobody"}),
            ]
            .iter()
            .map(|r| call(&h, r).unwrap())
            .collect::<Vec<_>>()
        })
        .await
        .unwrap();
        assert_eq!(answers[0], json!({"ok": true, "generation": 2}));
        assert_eq!(answers[1]["ok"], false);
        assert_eq!(answers[2]["ok"], false);
        // The running tally reaches its checkpoint file on request.
        assert_eq!(answers[3], json!({"ok": true, "written": 1}));
        assert_eq!(answers[4], json!({"ok": false, "error": "no record rq_nobody"}));
        let cp = nullrouter_engine::quota::history::tally_file(dir.path(), "p", "a").unwrap();
        assert!(std::fs::read_to_string(cp).unwrap().contains("\"requests\":1"));
        stop.send(()).unwrap();
        task.await.unwrap();
        assert!(!socket_path(&home).exists());
        assert!(matches!(call(&home, &json!({"op": "reload"})), Err(CallError::NoServer(_))));
    }
}
