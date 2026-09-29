//! The operator socket (contracts/operator-cli.md § Operator socket protocol): NDJSON over
//! a Unix socket at `$ZEROROUTER_HOME/run/operator.sock`, mode 0600 inside a 0700
//! directory. One request per line, one response per line.
//!
//! | Request | Response |
//! |---|---|
//! | `{"op":"reload"}` | `{"ok":true,"generation":N}`, or the error with the old snapshot kept |
//! | `{"op":"records.list","provider"?,"unified_model"?,"limit"?}` | `{"ok":true,"records":[…]}` |
//! | `{"op":"records.get","id":"rq_…"}` | `{"ok":true,"record":{…}}` |
//! | `{"op":"accounts.state"}` | `{"ok":true,"accounts":[…]}` |

use std::future::Future;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use zerorouter_engine::records::Query;
use zerorouter_engine::state::Engine;
use zerorouter_registry::OperatorHome;

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

/// One request's answer.
pub async fn handle(engine: &Arc<Engine>, req: &Value) -> Value {
    let str_of = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_owned);
    match req.get("op").and_then(Value::as_str) {
        Some("reload") => match engine.reload().await {
            Ok(r) => {
                for a in &r.unused_accounts {
                    tracing::warn!("account {a} names a provider that isn't loaded");
                }
                json!({"ok": true, "generation": r.generation})
            }
            Err(e) => {
                let error = engine.snapshot().redactor.redact(&e.to_string()).into_owned();
                tracing::error!("reload failed, the previous state stays: {error}");
                json!({"ok": false, "error": error})
            }
        },
        Some("records.list") => {
            let q = Query {
                provider: str_of("provider"),
                unified_model: str_of("unified_model"),
                limit: req.get("limit").and_then(Value::as_u64).map(|n| n as usize),
            };
            json!({"ok": true, "records": engine.records.query(&q)})
        }
        Some("records.get") => match str_of("id").and_then(|id| engine.records.get(&id)) {
            Some(r) => json!({"ok": true, "record": r}),
            None => json!({"ok": false, "error": "no such record (it may have been evicted)"}),
        },
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
                    json!({
                        "provider": a.provider,
                        "name": a.name,
                        "disabled": a.disabled,
                        "level": engine.cooldowns.level(&a.provider, &a.name),
                        "cooling": cooling,
                    })
                })
                .collect();
            json!({"ok": true, "accounts": accounts})
        }
        Some(op) => json!({"ok": false, "error": format!("unknown op {op:?}")}),
        None => json!({"ok": false, "error": "the request names no op"}),
    }
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
            [json!({"op": "reload"}), json!({"op": "records.get", "id": "rq_x"}), json!({"op": "nope"})]
                .iter()
                .map(|r| call(&h, r).unwrap())
                .collect::<Vec<_>>()
        })
        .await
        .unwrap();
        assert_eq!(answers[0], json!({"ok": true, "generation": 2}));
        assert_eq!(answers[1]["ok"], false);
        assert_eq!(answers[2]["ok"], false);
        stop.send(()).unwrap();
        task.await.unwrap();
        assert!(!socket_path(&home).exists());
        assert!(matches!(call(&home, &json!({"op": "reload"})), Err(CallError::NoServer(_))));
    }
}
