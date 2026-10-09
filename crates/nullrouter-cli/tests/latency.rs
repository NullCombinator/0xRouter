//! `nullrouter latency` (spec 010, US2, T022; contracts/cli.md): the text, the `--json`, the
//! `none` for a row with no values and the empty window, on a home with no server. The time and
//! the zone are pinned, so every line is exact.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const NOW: &str = "2026-10-06T14:02:11Z";

fn nr(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(args)
        .env("NULLROUTER_TEST_NOW", NOW)
        .env("TZ", "UTC")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Two requests from `ak_a` on `xai`: the first answered (overhead 4 ms, ttft 420 ms, so the
/// provider's own wait is 416 ms), the second failed with 503 and no first token.
fn home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("records")).unwrap();
    let open = |id: &str, at: &str| json!({"v":1,"t":"open","id":id,"arrived":at,"agent":"ak_a","style":"anthropic-messages","op":"generate","type":"text","target":"sonnet"});
    let attempt = |id: &str, outcome: Value| {
        json!({"v":1,"t":"attempt","id":id,"attempt":{"n":1,"provider":"xai","account":"work","model":"m","kind":"initial",
            "started":4.0,"ended":1000.0,"outcome":outcome,"dropped":[],"forced":[]}})
    };
    let close = |id: &str, outcome: &str, ttft: Value| {
        json!({"v":1,"t":"close","id":id,"outcome":outcome,"served_by":null,"ttft_ms":ttft,"total_ms":1000.0,
            "usage":null,"break_handling":{"kind":"none"},"job":null})
    };
    let lines = [
        open("rq_1", "2026-10-06T13:00:00Z"),
        attempt("rq_1", json!({"state":"ok"})),
        close("rq_1", "succeeded", json!(420.0)),
        open("rq_2", "2026-10-06T14:00:00Z"),
        attempt("rq_2", json!({"state":"failed","status":503,"reason":"overloaded"})),
        close("rq_2", "failed", Value::Null),
    ];
    let body: String = lines.iter().map(|l| l.to_string() + "\n").collect();
    std::fs::write(home.path().join("records/2026-10-06.jsonl"), body).unwrap();
    home
}

/// The lines with each run of spaces squeezed to one, so the column widths aren't pinned.
fn squeezed(text: &str) -> Vec<String> {
    text.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
}

#[test]
fn the_text_follows_the_contract() {
    let h = home();
    let o = nr(h.path(), &["latency"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let lines = squeezed(&out(&o));
    assert_eq!(lines[0], "last 24 h, 2026-10-05 14:02:11 → 2026-10-06 14:02:11 UTC");
    assert_eq!(lines[1], "");
    assert_eq!(lines[2], "agent requests overhead p50/p95 ttft p50/p95 last response");
    assert_eq!(lines[3], "ak_a 2 4 ms / 4 ms 420 ms / 420 ms failed 14:00:01");
    assert_eq!(lines[4], "");
    assert_eq!(lines[5], "provider requests own ttft p50/p95 last response by agent");
    assert_eq!(lines[6], "xai 2 416 ms / 416 ms failed 14:00:01 (503) ak_a 2");
}

#[test]
fn the_json_has_milliseconds_as_numbers() {
    let h = home();
    let o = nr(h.path(), &["--json", "latency"]);
    assert!(o.status.success());
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["window"], "last 24 h");
    assert_eq!(v["agents"][0]["id"], "ak_a");
    assert_eq!(v["agents"][0]["requests"], 2);
    assert_eq!(v["agents"][0]["overhead"]["p50"], 4.0);
    assert_eq!(v["agents"][0]["last"]["result"], "failed");
    assert_eq!(v["providers"][0]["id"], "xai");
    assert_eq!(v["providers"][0]["own_ttft"]["p50"], 416.0);
    assert_eq!(v["providers"][0]["last"]["status"], 503);
}

#[test]
fn an_empty_window_says_so() {
    let h = tempfile::tempdir().unwrap();
    let o = nr(h.path(), &["latency"]);
    assert!(o.status.success());
    let text = out(&o);
    assert!(text.lines().next().unwrap().starts_with("last 24 h, "), "{text}");
    assert!(text.contains("no requests in the last 24 h"), "{text}");
    assert!(!text.contains("requests  overhead"), "no tables: {text}");
}
