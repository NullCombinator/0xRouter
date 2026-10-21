//! The catalogue is contacted only by operator commands (contracts/catalogue.md, FR-031,
//! SC-012): a server with `catalogue_url` pointed at a counting listener makes no connection
//! to it through requests, a reload or a restart. Also no source file under the server and
//! engine names the catalogue module outside the install path.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{SECRET, reply_by_wire};
use nullrouter_engine::files;
use nullrouter_engine::keys::{self, Keys};
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::MockUpstream;
use nullrouter_registry::OperatorHome;
use nullrouter_server::serve::{App, run};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// A loopback listener that counts every connection it accepts and answers none. It is bound
/// before the server opens, so `catalogue_url` can name it. Any connection attempt counts, not
/// only a completed HTTP request.
async fn counting_listener() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let contacts = Arc::new(AtomicUsize::new(0));
    let counted = contacts.clone();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(socket);
        }
    });
    (port, contacts)
}

/// Writes `config.toml` (its catalogue URL names the counting listener), the `mockco` plugin
/// pointed at `mock`, an account and an agent key. Returns the key.
fn write_home(home: &Path, mock: &MockUpstream, catalogue_port: u16) -> String {
    let config = format!(
        "allow_private_endpoints = true\n[adapters]\ncatalogue_url = \"https://127.0.0.1:{catalogue_port}/index.json\"\n"
    );
    std::fs::write(home.join("config.toml"), config).unwrap();
    std::fs::create_dir(home.join("plugins")).unwrap();
    let plugin = format!(
        "schema = 2\nid = \"mockco\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n",
        mock.url("/v1/chat/completions")
    );
    std::fs::write(home.join("plugins/mockco.toml"), plugin).unwrap();
    let accounts = format!("schema = 1\n[[account]]\nprovider = \"mockco\"\nname = \"main\"\nsecret = \"{SECRET}\"\n");
    files::write_private(&home.join(nullrouter_engine::accounts::FILE), &accounts).unwrap();
    let mut keys = Keys::default();
    let (key, _) = keys.issue("laptop", None).unwrap();
    files::write_private(&home.join(keys::FILE), &keys.to_toml()).unwrap();
    key
}

/// Opens the engine over `home` as `nullrouter serve` does.
fn open(home: &Path) -> Arc<Engine> {
    let (engine, report) = Engine::open(OperatorHome::new(home)).unwrap();
    assert!(report.registry.diagnostics.is_empty(), "{:#?}", report.registry.diagnostics);
    engine.open_adapters().unwrap();
    Arc::new(engine)
}

/// A server on an ephemeral port, stopped by [`Running::stop`].
struct Running {
    base: String,
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}

async fn serve(engine: &Arc<Engine>) -> Running {
    let app = App::new(engine.clone()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(run(app, listener, async {
        let _ = stopped.await;
    }));
    Running { base, stop, task }
}

impl Running {
    async fn stop(self) {
        let _ = self.stop.send(());
        self.task.await.unwrap().unwrap();
    }
}

/// One chat completion through `mockco`, whole or streamed; returns the body.
async fn chat(base: &str, key: &str, stream: bool) -> String {
    let body = json!({"model": "mockco/m1", "stream": stream, "messages": [{"role": "user", "content": "hi"}]});
    let r = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(key)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    r.text().await.unwrap()
}

#[tokio::test]
async fn catalogue_is_never_contacted_by_requests_reload_or_restart() {
    let mock = MockUpstream::start().await;
    mock.respond(reply_by_wire);
    let (port, contacts) = counting_listener().await;
    let dir = tempfile::tempdir().unwrap();
    let key = write_home(dir.path(), &mock, port);

    let engine = open(dir.path());
    let s = serve(&engine).await;
    assert!(chat(&s.base, &key, false).await.contains("Hello"));
    assert!(chat(&s.base, &key, true).await.contains("Hel"));
    assert_eq!(contacts.load(Ordering::SeqCst), 0, "a request contacted the catalogue");

    engine.reload_blocking().unwrap();
    assert!(chat(&s.base, &key, false).await.contains("Hello"));
    assert_eq!(contacts.load(Ordering::SeqCst), 0, "a reload contacted the catalogue");

    s.stop().await;
    engine.journal.shutdown();
    drop(engine);

    let engine = open(dir.path());
    let s = serve(&engine).await;
    assert!(chat(&s.base, &key, true).await.contains("Hel"));
    assert_eq!(contacts.load(Ordering::SeqCst), 0, "a restart contacted the catalogue");
    s.stop().await;
    engine.journal.shutdown();
}

/// Source files under the server and engine crates name no `catalogue::` path: the catalogue
/// is reached from the operator CLI alone (FR-031).
#[test]
fn no_server_or_engine_source_names_the_catalogue_module() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut hits = Vec::new();
    for dir in [root.join("src"), root.join("../nullrouter-engine/src")] {
        scan(&dir, &mut hits);
    }
    assert!(hits.is_empty(), "catalogue:: named outside the install path (FR-031):\n{}", hits.join("\n"));
}

fn scan(dir: &Path, hits: &mut Vec<String>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, hits);
        } else if path.extension().and_then(|x| x.to_str()) == Some("rs") {
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            for (i, line) in text.lines().enumerate() {
                if line.contains("catalogue::") {
                    hits.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                }
            }
        }
    }
}
