//! `nullrouter verdicts` (spec 011 T043, US4 scenarios 1–4; contracts/cli.md § `nullrouter
//! verdicts`): the list and its filters, the operator's `mark` and `clear` with and without a
//! server, and the `[tests]` settings.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use serde_json::Value;

const SECRET: &str = "sk-verdicts-SENTINEL-0043";
const OPUS: &str = "claude-opus-4-1";

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
    String::from_utf8(o.stdout).unwrap()
}

fn refused(home: &Path, args: &[&str]) -> String {
    let o = nr(home, args);
    assert_eq!(o.status.code(), Some(1), "{args:?}: {}", text(&o));
    text(&o)
}

fn json(home: &Path, args: &[&str]) -> Value {
    let mut all = vec!["--json"];
    all.extend_from_slice(args);
    serde_json::from_str(&ok(home, &all)).unwrap()
}

/// Two bundled-anthropic accounts, `max` and `pro`.
fn home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    ok(dir.path(), &["accounts", "add", "anthropic", "max"]);
    ok(dir.path(), &["accounts", "add", "anthropic", "pro"]);
    dir
}

struct Serving(Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn serve(home: &Path) -> Serving {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let serving = Serving(child);
    for _ in 0..500 {
        if home.join("run/operator.sock").exists() && std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return serving;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the server did not start");
}

#[test]
fn the_operators_mark_survives_a_restart_and_shows_as_theirs() {
    let dir = home();
    let h = dir.path();
    assert_eq!(ok(h, &["verdicts"]), "no verdicts; every model is untested and routes normally\n");

    // No server: written to the file, applied at the next start.
    let out = ok(h, &["verdicts", "mark", "anthropic", "max", OPUS, "--note", "not on our plan"]);
    assert_eq!(out, format!("anthropic/max {OPUS}: BROKEN, set by the operator: saved; applies at next start\n"));
    ok(h, &["verdicts", "mark", "anthropic", "pro", OPUS]);

    let list = ok(h, &["verdicts"]);
    let lines: Vec<&str> = list.lines().collect();
    assert_eq!(lines.len(), 3, "{list}");
    assert!(lines[0].starts_with("provider   account  model"), "{list}");
    assert!(lines[1].contains("max      claude-opus-4-1  BROKEN"), "{list}");
    assert!(lines[1].ends_with("operator  set by the operator: not on our plan"), "{list}");
    assert!(lines[2].ends_with("operator  set by the operator"), "{list}");

    let all = json(h, &["verdicts"]);
    let v = &all["verdicts"][0];
    assert_eq!((v["state"].as_str(), v["source"].as_str()), (Some("broken"), Some("operator")));
    assert_eq!(v["note"], "not on our plan");
    assert!(v.get("basis").is_none(), "the basis stays in the file: {v}");
    assert!(v.get("next").is_none(), "an operator's BROKEN is never retested: {v}");

    // A start keeps it: its basis is the account's as the server computes it (research R7).
    let _serving = serve(h);
    let all = json(h, &["verdicts"]);
    assert_eq!(all["verdicts"].as_array().unwrap().len(), 2, "{all}");
    assert_eq!(all["verdicts"][0]["state"], "broken");
}

#[test]
fn the_list_filters_by_provider_account_model_and_state() {
    let dir = home();
    let h = dir.path();
    ok(h, &["verdicts", "mark", "anthropic", "max", OPUS]);
    ok(h, &["verdicts", "mark", "anthropic", "pro", "claude-sonnet-4-5"]);
    let count = |args: &[&str]| json(h, args)["verdicts"].as_array().unwrap().len();
    assert_eq!(count(&["verdicts"]), 2);
    assert_eq!(count(&["verdicts", "--account", "pro"]), 1);
    assert_eq!(count(&["verdicts", "--model", OPUS]), 1);
    assert_eq!(count(&["verdicts", "--provider", "anthropic", "--state", "broken"]), 2);
    assert_eq!(count(&["verdicts", "--state", "pass"]), 0);
    assert_eq!(count(&["verdicts", "--provider", "openai"]), 0);
    let none = ok(h, &["verdicts", "--state", "unknown"]);
    assert_eq!(none, "no verdicts; every model is untested and routes normally\n");
}

#[test]
fn clear_returns_a_pair_to_untested_and_refuses_an_untested_one() {
    let dir = home();
    let h = dir.path();
    ok(h, &["verdicts", "mark", "anthropic", "max", OPUS]);
    let _serving = serve(h);
    let cleared = ok(h, &["verdicts", "clear", "anthropic", "max", OPUS]);
    assert_eq!(cleared, format!("anthropic/max {OPUS}: untested: applied\n"));
    assert_eq!(json(h, &["verdicts"])["verdicts"], serde_json::json!([]));
    let e = refused(h, &["verdicts", "clear", "anthropic", "max", OPUS]);
    assert!(e.contains(&format!("no verdict for anthropic/max {OPUS}")), "{e}");

    // Through the server too: marked, then listed by the running server.
    assert!(ok(h, &["verdicts", "mark", "anthropic", "pro", OPUS, "--note", "x"]).ends_with(": applied\n"));
    assert_eq!(json(h, &["verdicts", "--account", "pro"])["verdicts"][0]["reason"], "set by the operator: x");
    let e = refused(h, &["verdicts", "mark", "anthropic", "nobody", OPUS]);
    assert!(e.contains("no account anthropic/nobody"), "{e}");
}

#[test]
fn settings_show_and_set_each_value_and_refuse_a_broken_rule() {
    let dir = home();
    let h = dir.path();
    // Each row with its columns one space apart.
    let rows = |h: &Path| -> Vec<String> {
        ok(h, &["verdicts", "settings"]).lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
    };
    let shown = rows(h);
    assert_eq!(shown[0], "setting value default");
    assert!(shown.contains(&"retest 1m,5m,30m,6h 1m,5m,30m,6h".to_owned()), "{shown:?}");
    assert!(shown.contains(&"broken-retest off off".to_owned()), "{shown:?}");
    assert!(shown.contains(&"timeout image 5m 5m".to_owned()), "{shown:?}");
    assert!(shown.contains(&"timeout text 30s 30s".to_owned()), "{shown:?}");

    let saved = ": saved; applies at next start\n";
    assert_eq!(ok(h, &["verdicts", "settings", "retest", "2m,10m"]), format!("retest = 2m,10m{saved}"));
    assert_eq!(ok(h, &["verdicts", "settings", "broken-retest", "on"]), format!("broken-retest = on{saved}"));
    assert_eq!(ok(h, &["verdicts", "settings", "timeout", "image", "10m"]), format!("timeout = image 10m{saved}"));
    let config = std::fs::read_to_string(h.join("config.toml")).unwrap();
    assert!(config.contains("[tests]\nbroken_retest = \"on\"\nretest = [\"2m\", \"10m\"]\n"), "{config}");
    assert!(config.contains("[tests.timeout]\nimage = \"10m\"\n"), "{config}");
    let j = json(h, &["verdicts", "settings"]);
    assert_eq!(j["retest"], serde_json::json!({"value": "2m,10m", "default": "1m,5m,30m,6h"}));
    assert_eq!(j["broken-retest"]["value"], "24h");
    assert_eq!(j["timeout image"]["value"], "10m");

    for (args, rule) in [
        (&["retest", "10s"][..], "at least 30s"),
        (&["retest", "5m,1m"][..], "steps must not get shorter"),
        (&["broken-retest", "10m"][..], "\"off\", \"on\" or at least 1h"),
        (&["concurrency", "40"][..], "1 to 32"),
        (&["timeout", "video", "1h"][..], "5s to 30m"),
        (&["timeout", "music", "1m"][..], "the type is one of"),
    ] {
        let mut all = vec!["verdicts", "settings"];
        all.extend_from_slice(args);
        let e = refused(h, &all);
        assert!(e.contains(rule), "{args:?}: {e}");
    }
    assert_eq!(std::fs::read_to_string(h.join("config.toml")).unwrap(), config, "a refused value writes nothing");

    assert_eq!(ok(h, &["verdicts", "settings", "retest", "default"]), format!("retest = default{saved}"));
    let _serving = serve(h);
    assert_eq!(ok(h, &["verdicts", "settings", "concurrency", "2"]), "concurrency = 2: applied\n");
    assert!(rows(h).contains(&"concurrency 2 4".to_owned()));
}

#[test]
fn unified_shows_each_members_verdicts_per_account() {
    let dir = home();
    let h = dir.path();
    let config = "[[unified_model]]\nname = \"opus\"\n\
                  members = [{ provider = \"anthropic\", model = \"claude-opus-4-20250514\" }, \
                  { provider = \"openrouter\", model = \"openai/gpt-5\" }]\n";
    std::fs::write(h.join("config.toml"), config).unwrap();
    let plain = ok(h, &["unified", "opus"]);
    assert!(!plain.contains("verdicts"), "no column while every member is untested: {plain}");

    let entry = json(h, &["unified", "opus"]);
    let upstream = |i: usize| entry["members"][i]["upstream_id"].as_str().unwrap().to_owned();
    ok(h, &["verdicts", "mark", "anthropic", "max", &upstream(0)]);
    let shown = ok(h, &["unified", "opus"]);
    let lines: Vec<&str> = shown.lines().collect();
    assert!(lines[1].ends_with(&format!("→ upstream {}  verdicts: max BROKEN", upstream(0))), "{shown}");
    assert!(lines[2].ends_with(&format!("→ upstream {}  verdicts: untested", upstream(1))), "{shown}");
    assert_eq!(json(h, &["unified", "opus"]), entry, "the JSON is resolve's, unchanged");
}
