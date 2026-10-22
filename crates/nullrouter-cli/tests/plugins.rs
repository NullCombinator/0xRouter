//! `nullrouter plugins` (T128, US8-1): the community set with its fit status, install that
//! refuses an unsupported plugin with exit 3, and an installed plugin that serves requests.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use nullrouter_engine::testkit::{MockUpstream, Step};
use serde_json::{Value, json};

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

fn json_out(home: &Path, args: &[&str]) -> Value {
    let o = nr(home, args, "");
    assert!(o.status.success(), "{args:?}: {}", text(&o));
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
fn list_install_refuse_and_uninstall() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();

    let rows = json_out(home, &["--json", "plugins", "list", "--community"]);
    let rows = rows.as_array().unwrap();
    let community: Vec<&Value> = rows.iter().filter(|r| r["set"] == "community").collect();
    assert_eq!(community.len(), 114);
    assert!(
        community.iter().all(|r| ["fits", "unsupported"].contains(&r["status"].as_str().unwrap())),
        "{community:?}"
    );
    assert_eq!(rows.iter().filter(|r| r["set"] == "bundled").count(), 7);
    let without = json_out(home, &["--json", "plugins", "list"]);
    assert!(without.as_array().unwrap().iter().all(|r| r["set"] != "community"));

    let o = nr(home, &["plugins", "install", "qoder"], "");
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("not supported by this core") && err.ends_with("No part of this plugin was loaded.\n"),
        "{err}"
    );
    assert!(!home.join("plugins/qoder.toml").exists());
    assert_eq!(nr(home, &["plugins", "install", "no-such-plugin"], "").status.code(), Some(1));

    let o = nr(home, &["plugins", "install", "groq"], "");
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("saved; applies at next start"), "{}", text(&o));
    let providers = json_out(home, &["--json", "providers"]);
    let groq = providers.as_array().unwrap().iter().find(|p| p["id"] == "groq").expect("groq is listed");
    assert_eq!(groq["source"], "user");
    let rows = json_out(home, &["--json", "plugins", "list", "--community"]);
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == "groq" && r["set"] == "community" && r["status"] == "installed")
    );
    assert_eq!(nr(home, &["plugins", "install", "groq"], "").status.code(), Some(1), "already installed");

    assert!(nr(home, &["plugins", "uninstall", "groq"], "").status.success());
    assert!(!home.join("plugins/groq.toml").exists());
    assert_eq!(nr(home, &["plugins", "uninstall", "groq"], "").status.code(), Some(1));
}

struct Serving(Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// An installed community plugin serves a request once an account is added. Its URL is
/// pointed at the mock first: the test can't reach the provider.
#[tokio::test(flavor = "multi_thread")]
async fn an_installed_plugin_serves_a_request() {
    let mock = MockUpstream::start().await;
    mock.respond(|_| {
        Step::json(
            200,
            json!({"id": "up-1", "object": "chat.completion", "created": 1, "model": "m", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi from groq"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 3, "completion_tokens": 3}}),
        )
    });
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::write(home.join("config.toml"), "allow_private_endpoints = true\n").unwrap();
    assert!(nr(home, &["plugins", "install", "groq"], "").status.success());
    let file = home.join("plugins/groq.toml");
    let src = std::fs::read_to_string(&file).unwrap();
    let upstream = "https://api.groq.com/openai/v1/chat/completions";
    assert!(src.contains(upstream));
    std::fs::write(&file, src.replace(upstream, &mock.url("/groq/chat/completions"))).unwrap();
    let o = nr(home, &["accounts", "add", "groq", "main"], "gsk-SENTINEL-0128\n");
    assert!(o.status.success(), "{}", text(&o));
    let key = String::from_utf8(nr(home, &["keys", "issue", "t"], "").stdout).unwrap().trim().to_owned();

    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let listen = format!("127.0.0.1:{port}");
    let _serving = Serving(
        Command::new(env!("CARGO_BIN_EXE_nullrouter"))
            .arg("--home")
            .arg(home)
            .args(["serve", "--listen", &listen])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    for _ in 0..500 {
        if tokio::net::TcpStream::connect(&listen).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The first catalogued model.
    let first = src.split("[[models]]").nth(1).unwrap();
    let model = first.lines().find_map(|l| l.strip_prefix("id = \"")).unwrap().trim_end_matches('"');
    let r = reqwest::Client::new()
        .post(format!("http://{listen}/v1/chat/completions"))
        .bearer_auth(&key)
        .body(json!({"model": format!("groq/{model}"), "messages": [{"role": "user", "content": "hi"}]}).to_string())
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let body: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["choices"][0]["message"]["content"], "hi from groq");
    let got = mock.received();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].headers["authorization"], "Bearer gsk-SENTINEL-0128");
}

#[test]
fn private_endpoints_asks_unless_yes_and_edits_config() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let cfg = home.join("config.toml");
    let o = nr(home, &["plugins", "private-endpoints", "on"], "n\n");
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(
        text(&o).contains("Plugins may then reach this machine and your local network. Continue? [y/N]"),
        "{}",
        text(&o)
    );
    assert!(!cfg.exists());
    let o = nr(home, &["plugins", "private-endpoints", "on"], "y\n");
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("saved; applies at next start"), "{}", text(&o));
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "allow_private_endpoints = true\n");
    // Off needs no question.
    assert!(nr(home, &["plugins", "private-endpoints", "off"], "").status.success());
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "");
    assert!(nr(home, &["plugins", "private-endpoints", "on", "--yes"], "").status.success());
}

#[test]
fn add_compatible_writes_a_provider_and_refuses_private_addresses() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let add = |id: &str, style: &str, url: &str| {
        nr(home, &["providers", "add-compatible", id, "--style", style, "--base-url", url, "--model", "a", "--model", "b", "--model", "c"], "")
    };
    let o = add("my-openai", "openai", "https://llm.example.com/v1");
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("my-openai: added (3 models), saved; applies at next start"), "{}", text(&o));
    assert!(add("my-claude", "anthropic", "https://claude.example.com").status.success());
    let rows = json_out(home, &["--json", "providers"]);
    assert!(rows.as_array().unwrap().iter().any(|p| p["id"] == "my-openai" && p["source"] == "user"));
    assert!(rows.as_array().unwrap().iter().any(|p| p["id"] == "my-claude"));

    // A taken id, a bad URL, a private URL.
    assert_eq!(add("my-openai", "openai", "https://llm.example.com/v1").status.code(), Some(1));
    assert_eq!(add("openrouter", "openai", "https://llm.example.com/v1").status.code(), Some(1));
    assert_eq!(add("bad-url", "openai", "not a url").status.code(), Some(1));
    let o = add("local-llm", "openai", "http://127.0.0.1:11434");
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr).trim(),
        "local-llm: base_url http://127.0.0.1:11434 is a private address; turn private endpoints on first (nullrouter plugins private-endpoints on)"
    );
    assert!(!home.join("plugins/local-llm.toml").exists());
    assert!(nr(home, &["plugins", "private-endpoints", "on", "--yes"], "").status.success());
    assert!(add("local-llm", "openai", "http://127.0.0.1:11434").status.success());
    // The plain list still works.
    assert!(nr(home, &["providers"], "").status.success());
}
