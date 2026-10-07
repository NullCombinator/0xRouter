//! `nullrouter test` against a running server (spec 011 T023, SC-004): output lines, the
//! confirmation, `--yes`, `--json`, no server, a hang ending within its timeout, and an
//! interrupted run keeping what finished while sending nothing new.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use nullrouter_engine::testkit::{MockUpstream, Received, Step};
use serde_json::{Value, json};

const SECRET: &str = "sk-test-SENTINEL-0110";

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
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    child.wait_with_output().unwrap()
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn plugin(mock: &MockUpstream, id: &str) -> String {
    format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n",
        mock.url(&format!("/{id}/chat/completions"))
    )
}

fn reply(r: &Received) -> Step {
    if r.path_and_query.starts_with("/slow") {
        return Step::StallHeaders { hold: Duration::from_secs(60) };
    }
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

/// A home with providers `ids` (one account each) under the unified model `u`, and `config`.
fn home_with(mock: &MockUpstream, ids: &[&str], config: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let members: Vec<String> = ids.iter().map(|id| format!("{{ provider = \"{id}\", model = \"m1\" }}")).collect();
    std::fs::write(
        home.join("config.toml"),
        format!(
            "allow_private_endpoints = true\n{config}\n[[unified_model]]\nname = \"u\"\nmembers = [{}]\n",
            members.join(", ")
        ),
    )
    .unwrap();
    std::fs::create_dir(home.join("plugins")).unwrap();
    for id in ids {
        std::fs::write(home.join(format!("plugins/{id}.toml")), plugin(mock, id)).unwrap();
        let o = nr(home, &["accounts", "add", id, "main"], &format!("{SECRET}-{id}\n"));
        assert!(o.status.success(), "{}", text(&o));
    }
    dir
}

async fn serve(home: &Path) -> Serving {
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
            return serving;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the server never came up");
}

fn verdict_lines(home: &Path) -> Vec<Value> {
    std::fs::read_to_string(home.join("routing/verdicts.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_server_nothing_is_called() {
    let mock = MockUpstream::start().await;
    mock.respond(reply);
    let dir = home_with(&mock, &["alpha"], "");
    let o = nr(dir.path(), &["test", "alpha/m1"], "");
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("no running server; start it with nullrouter serve"), "{}", text(&o));
    assert!(mock.received().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn asks_before_more_than_one_call_then_prints_each_result() {
    let mock = MockUpstream::start().await;
    mock.respond(reply);
    let dir = home_with(&mock, &["alpha", "beta"], "");
    let home = dir.path();
    let _server = serve(home).await;

    let o = nr(home, &["test", "u"], "n\n");
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("2 billed calls: 2 text. Continue? [y/N]"), "{}", text(&o));
    assert!(mock.received().is_empty(), "declined: no call");

    let o = nr(home, &["test", "u", "--yes"], "");
    assert!(o.status.success(), "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out}");
    for who in ["alpha/main", "beta/main"] {
        assert!(lines.iter().any(|l| l.starts_with("PASS") && l.contains(who) && l.contains("m1")), "{out}");
    }
    assert_eq!(lines[2], "2 pairs: 2 pass, 0 broken, 0 unknown, 0 skipped");
    assert_eq!(mock.received().len(), 2);

    // One pair: no question.
    let o = nr(home, &["--json", "test", "alpha/m1"], "");
    assert!(o.status.success(), "{}", text(&o));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["done"]["pass"], 1, "{v}");
    assert_eq!(v["results"][0]["state"], "pass");
    assert!(v["results"][0]["record"].as_str().unwrap().starts_with("rq_"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hang_returns_within_its_timeout() {
    let mock = MockUpstream::start().await;
    mock.respond(reply);
    let dir = home_with(&mock, &["slow"], "[tests.timeout]\ntext = \"5s\"\n");
    let home = dir.path();
    let _server = serve(home).await;
    let started = Instant::now();
    let o = nr(home, &["test", "slow/m1"], "");
    assert!(started.elapsed() < Duration::from_secs(10), "SC-004: {:?}", started.elapsed());
    assert!(o.status.success(), "{}", text(&o));
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("UNKNOWN  slow/main"), "{}", text(&o));
}

/// Closing the CLI mid-run: the finished result and the call in flight are kept; the call not
/// yet sent is never made.
#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_run_keeps_what_finished_and_sends_nothing_new() {
    let mock = MockUpstream::start().await;
    mock.respond(reply);
    let dir = home_with(&mock, &["slow", "slower", "slowest"], "[tests]\nconcurrency = 1\n[tests.timeout]\ntext = \"5s\"\n");
    let home = dir.path().to_owned();
    let _server = serve(&home).await;

    let mut child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(&home)
        .args(["test", "u", "--yes"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut first).unwrap();
    assert!(first.starts_with("UNKNOWN"), "{first}");
    child.kill().unwrap();
    let _ = child.wait();

    // The call in flight runs out its 5 s; the third is never sent.
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert_eq!(mock.received().len(), 2, "{:?}", mock.received().iter().map(|r| &r.path_and_query).collect::<Vec<_>>());
    assert_eq!(verdict_lines(&home).len(), 2, "both finished results are saved");
}
