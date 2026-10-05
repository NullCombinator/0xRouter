//! The routing commands (spec 006, US4, T050; contracts/operator-cli.md): `accounts priority`, the
//! `priority` column of `accounts list`, `routing set`, `routing unset`, `routing window` and the
//! `routing` view. Files are written atomically and checked with the registry gate's rules; the
//! view needs a running server (exit 4 without one).

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_registry::schema::OperatorConfig;
use serde_json::{Value, json};

const SECRET: &str = "sk-routing-SENTINEL-0050";

fn nr(home: &Path, args: &[&str]) -> Output {
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
    let _ = child.stdin.take().unwrap().write_all(format!("{SECRET}\n").as_bytes());
    child.wait_with_output().unwrap()
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn ok(home: &Path, args: &[&str]) -> String {
    let o = nr(home, args);
    assert!(o.status.success(), "{args:?}: {}", text(&o));
    text(&o)
}

fn refused(home: &Path, args: &[&str]) -> String {
    let o = nr(home, args);
    assert_eq!(o.status.code(), Some(1), "{args:?}: {}", text(&o));
    text(&o)
}

/// Two bundled-anthropic accounts, `max` and `pro`.
fn home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    ok(dir.path(), &["accounts", "add", "anthropic", "max"]);
    ok(dir.path(), &["accounts", "add", "anthropic", "pro"]);
    dir
}

fn load(home: &Path) -> Accounts {
    Accounts::load(&home.join(accounts::FILE)).unwrap()
}

#[test]
fn priority_is_saved_listed_and_refused_when_not_a_number() {
    let dir = home();
    let out = ok(dir.path(), &["accounts", "priority", "anthropic", "pro", "0"]);
    assert!(out.contains("anthropic/pro") && out.contains("saved; applies at next start"), "{out}");
    assert_eq!(load(dir.path()).get("anthropic", "pro").unwrap().priority, 0.0);
    ok(dir.path(), &["accounts", "priority", "anthropic", "max", "2.5"]);
    assert_eq!(load(dir.path()).get("anthropic", "max").unwrap().priority, 2.5);

    let list = ok(dir.path(), &["accounts", "list"]);
    let header = list.lines().next().unwrap();
    assert!(header.contains("priority"), "{header}");
    let max = list.lines().find(|l| l.contains("anthropic") && l.contains("max")).unwrap();
    assert!(max.contains("2.5"), "{max}");
    let rows: Value = serde_json::from_str(&ok(dir.path(), &["--json", "accounts", "list"])).unwrap();
    assert_eq!((rows[0]["priority"].as_f64(), rows[1]["priority"].as_f64()), (Some(2.5), Some(0.0)));

    for bad in ["-1", "soon", "NaN"] {
        let msg = refused(dir.path(), &["accounts", "priority", "anthropic", "pro", bad]);
        assert!(msg.contains("priority"), "{bad}: {msg}");
    }
    assert!(!refused(dir.path(), &["accounts", "priority", "anthropic", "nobody", "1"]).is_empty());
    assert_eq!(load(dir.path()).get("anthropic", "pro").unwrap().priority, 0.0, "refusals changed nothing");
}

#[test]
fn routing_set_and_unset_write_the_overrides_and_the_gate_refuses_bad_ones() {
    let dir = home();
    let out = ok(
        dir.path(),
        &[
            "routing",
            "set",
            "anthropic",
            "pro",
            "cache_lifetime=1h",
            "reserve=10%",
            "window.5-hour.capacity=12000000",
            "window.weekly.reserve=15%",
            "price.input=3.5",
            "price.output=14",
        ],
    );
    assert!(out.contains("saved; applies at next start"), "{out}");
    let list = load(dir.path());
    let r = &list.get("anthropic", "pro").unwrap().routing;
    assert_eq!(r.cache_lifetime, Some(Duration::from_secs(3600)));
    assert_eq!(r.reserve.map(|p| p.0), Some(10.0));
    assert_eq!(r.window["5-hour"].capacity, Some(12_000_000.0));
    assert_eq!(r.window["weekly"].reserve.map(|p| p.0), Some(15.0));
    let price = r.price.unwrap();
    assert_eq!((price.input, price.output), (3.5, Some(14.0)));
    assert!(list.get("anthropic", "max").unwrap().routing.is_empty(), "only the named account changes");
    assert!(!std::fs::read_to_string(dir.path().join(accounts::FILE)).unwrap().contains("tmp"));

    // `routing set` merges: what isn't named stays.
    ok(dir.path(), &["routing", "set", "anthropic", "pro", "cache_lifetime=30m"]);
    let r = load(dir.path()).get("anthropic", "pro").unwrap().routing.clone();
    assert_eq!(r.cache_lifetime, Some(Duration::from_secs(1800)));
    assert_eq!(r.reserve.map(|p| p.0), Some(10.0));

    ok(dir.path(), &["routing", "unset", "anthropic", "pro", "reserve", "window.5-hour", "price", "window.weekly.reserve"]);
    let r = load(dir.path()).get("anthropic", "pro").unwrap().routing.clone();
    assert_eq!(r.cache_lifetime, Some(Duration::from_secs(1800)));
    assert!(r.reserve.is_none() && r.price.is_none() && r.window.is_empty(), "{r:?}");
    ok(dir.path(), &["routing", "unset", "anthropic", "pro", "cache_lifetime"]);
    assert!(load(dir.path()).get("anthropic", "pro").unwrap().routing.is_empty());

    // Refusals say why, change nothing, and exit 1.
    let before = std::fs::read(dir.path().join(accounts::FILE)).unwrap();
    for (arg, why) in [
        ("cache_lifetime=48h", "24h"),
        ("cache_lifetime=soon", "cache_lifetime"),
        ("reserve=80%", "reserve"),
        ("reserve=10", "%"),
        ("window.5-hour.capacity=0", "capacity"),
        ("window.nonesuch.capacity=5", "nonesuch"),
        ("price.input=-1", "price"),
        ("colour=blue", "colour"),
        ("cache_lifetime", "="),
    ] {
        let msg = refused(dir.path(), &["routing", "set", "anthropic", "pro", arg]);
        assert!(msg.contains(why), "{arg}: {msg}");
    }
    assert!(!refused(dir.path(), &["routing", "set", "anthropic", "nobody", "reserve=5%"]).is_empty());
    assert!(!refused(dir.path(), &["routing", "unset", "anthropic", "pro", "colour"]).is_empty());
    assert_eq!(std::fs::read(dir.path().join(accounts::FILE)).unwrap(), before);
}

fn config(home: &Path) -> OperatorConfig {
    toml::from_str(&std::fs::read_to_string(home.join("config.toml")).unwrap()).unwrap()
}

#[test]
fn routing_window_sets_the_default_or_a_targets_length() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "# my settings\nschema = 1\n\n[pipeline]\nbreak_behaviour = \"restart\"\n").unwrap();
    let out = ok(dir.path(), &["routing", "window", "2h"]);
    assert!(out.contains("saved; applies at next start"), "{out}");
    assert_eq!(config(dir.path()).routing.amortization, Duration::from_secs(2 * 3600));
    ok(dir.path(), &["routing", "window", "sonnet", "1h"]);
    ok(dir.path(), &["routing", "window", "anthropic/claude-opus-4-1", "24h"]);
    let c = config(dir.path());
    assert_eq!(c.routing.amortization_for["sonnet"], Duration::from_secs(3600));
    assert_eq!(c.routing.amortization_for["anthropic/claude-opus-4-1"], Duration::from_secs(86_400));
    let file = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(file.starts_with("# my settings\n") && file.contains("break_behaviour = \"restart\""), "{file}");

    ok(dir.path(), &["routing", "window", "sonnet", "default"]);
    assert!(!config(dir.path()).routing.amortization_for.contains_key("sonnet"));
    assert!(config(dir.path()).routing.amortization_for.contains_key("anthropic/claude-opus-4-1"));
    ok(dir.path(), &["routing", "window", "default"]);
    assert_eq!(config(dir.path()).routing.amortization, Duration::from_secs(5 * 3600));

    let before = std::fs::read(dir.path().join("config.toml")).unwrap();
    for bad in [&["routing", "window", "0s"][..], &["routing", "window", "soon"], &["routing", "window", "sonnet", "0m"]] {
        let msg = refused(dir.path(), bad);
        assert!(msg.contains("amortization") || msg.contains("duration") || msg.contains("0"), "{bad:?}: {msg}");
    }
    assert_eq!(std::fs::read(dir.path().join("config.toml")).unwrap(), before);
}

#[test]
fn the_view_needs_a_server() {
    let dir = home();
    let o = nr(dir.path(), &["routing"]);
    assert_eq!(o.status.code(), Some(4), "{}", text(&o));
    assert!(text(&o).contains("no server is running"), "{}", text(&o));
}

// ---- against a running server ----

struct Serving(Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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

fn plugin(mock: &MockUpstream, id: &str, extra: &str) -> String {
    format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n{extra}",
        mock.url(&format!("/{id}/chat/completions"))
    )
}

const SUB: &str = "[routing.cache]\nmode = \"automatic\"\nlifetime = \"5m\"\nmin_tokens = 0\n\n[[routing.window]]\nname = \"5h\"\nlength = \"5h\"\nunit = \"weighted_tokens\"\ncapacity = 1000000\n";
const PAY: &str = "[[routing.price]]\ninput = 3.0\noutput = 12.0\n";

#[tokio::test(flavor = "multi_thread")]
async fn the_view_prints_the_contracts_columns_and_a_change_applies_to_the_running_server() {
    let mock = MockUpstream::start().await;
    mock.respond(|_| Step::json(200, json!({"id": "x", "object": "chat.completion", "model": "m1", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 3, "completion_tokens": 1}})));
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::write(
        home.join("config.toml"),
        "allow_private_endpoints = true\n[[unified_model]]\nname = \"u\"\nmembers = [{ provider = \"alpha\", model = \"m1\" }, { provider = \"pay\", model = \"m1\" }]\n",
    )
    .unwrap();
    std::fs::create_dir(home.join("plugins")).unwrap();
    std::fs::write(home.join("plugins/alpha.toml"), plugin(&mock, "alpha", SUB)).unwrap();
    std::fs::write(home.join("plugins/pay.toml"), plugin(&mock, "pay", PAY)).unwrap();
    ok(home, &["accounts", "add", "alpha", "a"]);
    ok(home, &["accounts", "add", "alpha", "b"]);
    ok(home, &["accounts", "add", "pay", "key"]);
    ok(home, &["accounts", "priority", "alpha", "b", "0"]);
    let _server = serve(home).await;

    let view = ok(home, &["routing"]);
    let lines: Vec<&str> = view.lines().collect();
    assert!(lines[0].starts_with("amortization 5h (") && lines[0].contains("records kept"), "{view}");
    for want in ["subscription tier", "pay-as-you-go tier", "account", "source", "pace", "share", "deficit", "priority", "cache", "windows"] {
        assert!(view.contains(want), "{want:?} missing from:\n{view}");
    }
    let a = lines.iter().find(|l| l.contains("alpha/a")).unwrap();
    assert!(a.contains("estimated") && a.contains("5h") && a.contains("wtok") && a.contains("floor 5%"), "{a}");
    assert!(a.contains("1000.0k") || a.contains("1.0M"), "{a}");
    let b = lines.iter().find(|l| l.contains("alpha/b")).unwrap();
    assert!(b.contains("cold work off (priority 0)"), "{b}");
    let p = lines.iter().find(|l| l.contains("pay/key")).unwrap();
    assert!(p.contains("payg") && p.contains("price now 3.00/Mtok in"), "{p}");
    assert!(!view.contains(SECRET));

    let json: Value = serde_json::from_str(&ok(home, &["--json", "routing", "u"])).unwrap();
    assert_eq!(json["targets"][0]["target"], "u");
    assert_eq!(json["journal"]["kept"], true);
    let first = &json["targets"][0]["accounts"][0]["windows"][0];
    for k in ["unit", "remaining_at_poll", "cost_since_poll", "remaining_now", "capacity", "reserve", "resets_at"] {
        assert!(first.get(k).is_some(), "{k} missing from {first}");
    }
    assert_eq!(json["targets"][0]["accounts"][0]["cache_lifetime"], "5m");

    // A change reaches the running server at once.
    let out = ok(home, &["accounts", "priority", "alpha", "b", "2"]);
    assert!(out.contains("applied"), "{out}");
    let out = ok(home, &["routing", "set", "alpha", "a", "cache_lifetime=1h"]);
    assert!(out.contains("applied"), "{out}");
    let json: Value = serde_json::from_str(&ok(home, &["--json", "routing"])).unwrap();
    let rows = json["targets"][0]["accounts"].as_array().unwrap();
    let get = |n: &str| rows.iter().find(|r| r["account"] == n && r["provider"] == "alpha").unwrap();
    assert_eq!(get("b")["priority"], 2.0);
    assert_eq!(get("a")["cache_lifetime"], "1h");
    let out = ok(home, &["routing", "window", "u", "1h"]);
    assert!(out.contains("applied"), "{out}");
    let json: Value = serde_json::from_str(&ok(home, &["--json", "routing", "u"])).unwrap();
    assert_eq!(json["targets"][0]["amortization_window"]["length"], "1h");
}
