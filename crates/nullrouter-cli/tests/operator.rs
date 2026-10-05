//! The CLI against a running `nullrouter serve` (T103, US5-3, US5-4): records are read over
//! the operator socket, account and key changes apply to the next request without a
//! restart, and listings show only a secret's last four characters. With no server,
//! `records` exits 4 and changes are saved for the next start.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use nullrouter_engine::testkit::{MockUpstream, Received, Step};
use serde_json::{Value, json};

const SECRET: &str = "sk-op-SENTINEL-0103";

fn nr(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A command that exits without reading stdin closes the pipe first; that is not a failure.
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    child.wait_with_output().unwrap()
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// Runs a command that must succeed and returns its stdout and stderr.
fn ok(home: &Path, args: &[&str], stdin: &str) -> String {
    let o = nr(home, args, stdin);
    assert!(o.status.success(), "{args:?}: {}", text(&o));
    text(&o)
}

fn plugin(mock: &MockUpstream, id: &str) -> String {
    format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n",
        mock.url(&format!("/{id}/chat/completions"))
    )
}

fn reply(_: &Received) -> Step {
    Step::json(
        200,
        json!({"id": "up-1", "object": "chat.completion", "created": 1, "model": "m1", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 3, "completion_tokens": 1}}),
    )
}

struct Serving(Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn serve(home: &Path) -> (Serving, String) {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let listen = format!("127.0.0.1:{port}");
    let child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(["serve", "--listen", &listen])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let serving = Serving(child);
    for _ in 0..500 {
        if home.join("run/operator.sock").exists() && tokio::net::TcpStream::connect(&listen).await.is_ok() {
            return (serving, format!("http://{listen}"));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the server never came up");
}

/// Sends a chat request for `model` with `key`; returns the status and the record id.
async fn send(base: &str, key: &str, model: &str) -> (u16, String) {
    let r = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(key)
        .body(json!({"model": model, "messages": [{"role": "user", "content": "hi"}]}).to_string())
        .send()
        .await
        .unwrap();
    let id = r.headers().get("x-0router-request-id").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    (r.status().as_u16(), id)
}

fn ids(list: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(list).unwrap();
    v.as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cli_reads_records_and_applies_changes_to_a_running_server() {
    let mock = MockUpstream::start().await;
    mock.respond(reply);
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::write(
        home.join("config.toml"),
        "allow_private_endpoints = true\n[[unified_model]]\nname = \"u\"\nmembers = [{ provider = \"alpha\", model = \"m1\" }, { provider = \"beta\", model = \"m1\" }]\n",
    )
    .unwrap();
    std::fs::create_dir(home.join("plugins")).unwrap();
    for id in ["alpha", "beta"] {
        std::fs::write(home.join(format!("plugins/{id}.toml")), plugin(&mock, id)).unwrap();
    }
    let mut outputs = Vec::new();

    // No server yet: changes are saved for the next start, records are read from disk (none yet).
    let out = ok(home, &["accounts", "add", "alpha", "main"], &format!("{SECRET}\n"));
    assert!(out.contains("saved; applies at next start"), "{out}");
    let o = nr(home, &["keys", "issue", "laptop"], "");
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("saved; applies at next start"), "{}", text(&o));
    let key = String::from_utf8(o.stdout).unwrap().trim().to_owned();
    let o = nr(home, &["records", "list"], "");
    assert!(o.status.success(), "{}", text(&o));
    outputs.push(text(&o));

    let (_server, base) = serve(home).await;
    assert_eq!(send(&base, &key, "alpha/m1").await.0, 200);
    let (status, _) = send(&base, &key, "beta/m1").await;
    assert_ne!(status, 200, "beta has no account yet");

    // An account added while serving takes the next request.
    let out = ok(home, &["accounts", "add", "beta", "main"], &format!("{SECRET}-beta\n"));
    assert!(out.contains("beta/main: applied"), "{out}");
    let (status, beta_id) = send(&base, &key, "beta/m1").await;
    assert_eq!(status, 200);
    let (status, unified_id) = send(&base, &key, "u").await;
    assert_eq!(status, 200);

    // Records by provider and by unified model.
    let beta = ids(&ok(home, &["--json", "records", "list", "--provider", "beta"], ""));
    assert_eq!(beta, vec![beta_id.clone()], "only the request beta served");
    let unified = ids(&ok(home, &["--json", "records", "list", "--model", "u"], ""));
    assert_eq!(unified, vec![unified_id.clone()]);
    let all = ok(home, &["records", "list"], "");
    assert_eq!(all.lines().count(), 4, "{all}");
    let show = ok(home, &["records", "show", &beta_id], "");
    for want in [
        beta_id.as_str(),
        "succeeded",
        "door        openai-chat",
        "served by   beta/main  m1",
        "usage       input 3  output 1",
        "attempts",
    ] {
        assert!(show.contains(want), "{want:?} missing from:\n{show}");
    }
    let show: Value = serde_json::from_str(&ok(home, &["--json", "records", "show", &unified_id], "")).unwrap();
    assert_eq!(show["unified_model"], "u");
    let o = nr(home, &["records", "show", "rq_nope"], "");
    assert!(!o.status.success());
    outputs.extend([all, show.to_string()]);

    // Disable, enable, remove: each applies to the next request.
    assert!(ok(home, &["accounts", "disable", "alpha", "main"], "").contains("applied"));
    assert_ne!(send(&base, &key, "alpha/m1").await.0, 200, "a disabled account serves nothing");
    let list = ok(home, &["accounts", "list"], "");
    assert!(list.contains("disabled"), "{list}");
    outputs.push(list);
    assert!(ok(home, &["accounts", "enable", "alpha", "main"], "").contains("applied"));
    assert_eq!(send(&base, &key, "alpha/m1").await.0, 200);
    assert!(ok(home, &["accounts", "remove", "beta", "main"], "").contains("applied"));
    assert_ne!(send(&base, &key, "beta/m1").await.0, 200, "a removed account serves nothing");

    // A key issued while serving works at once; revoked, it is refused at once.
    let o = nr(home, &["keys", "issue", "ci"], "");
    assert!(text(&o).contains("applied"), "{}", text(&o));
    let ci = String::from_utf8(o.stdout).unwrap().trim().to_owned();
    assert_eq!(send(&base, &ci, "alpha/m1").await.0, 200);
    assert!(ok(home, &["keys", "revoke", "ci"], "").contains("applied"));
    assert_eq!(send(&base, &ci, "alpha/m1").await.0, 401);
    assert_eq!(send(&base, &key, "alpha/m1").await.0, 200, "the other key still works");

    // Listings show only the last four characters of a secret or key.
    let accounts = ok(home, &["accounts", "list"], "");
    assert!(accounts.contains("…0103"), "{accounts}");
    let keys = ok(home, &["keys", "list"], "");
    assert!(keys.contains(&format!("…{}", &key[key.len() - 4..])), "{keys}");
    outputs.extend([accounts, keys]);
    for o in &outputs {
        for secret in [SECRET, key.as_str(), ci.as_str()] {
            assert!(!o.contains(secret), "CLI output shows a secret:\n{o}");
        }
    }
}

/// A plugin like `plugin`'s, with `m1` declaring `context_length`.
fn limited(mock: &MockUpstream, id: &str, context: u64) -> String {
    plugin(mock, id) + &format!("context_length = {context}\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_applied_to_a_running_server_prints_its_limits_notes() {
    let mock = MockUpstream::start().await;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::write(
        home.join("config.toml"),
        "allow_private_endpoints = true\n[[unified_model]]\nname = \"u\"\nmembers = [{ provider = \"alpha\", model = \"m1\" }, { provider = \"beta\", model = \"m1\" }]\n",
    )
    .unwrap();
    std::fs::create_dir(home.join("plugins")).unwrap();
    std::fs::write(home.join("plugins/alpha.toml"), limited(&mock, "alpha", 200_000)).unwrap();
    std::fs::write(home.join("plugins/beta.toml"), limited(&mock, "beta", 128_000)).unwrap();

    let (_server, _) = serve(home).await;
    let o = nr(home, &["accounts", "add", "alpha", "main"], &format!("{SECRET}\n"));
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "alpha/main: applied\n", "notes stay off stdout");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("note: unified model u: members differ in context_length: alpha 200000, beta 128000"), "{err}");
}
