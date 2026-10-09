//! `nullrouter usage` (spec 010, US1, T012; contracts/cli.md): the text, the `--json`, the empty
//! period and the unknown one, on a home with two records and no server. The time and the zone
//! are pinned, so every line is exact. (Not in `read_golden`: that gate's fixture homes would each
//! need blessed files; this reads one small home it builds itself.)

use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const NOW: &str = "2026-10-06T14:02:11Z";

fn nr(home: &Path, now: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(args)
        .env("NULLROUTER_TEST_NOW", now)
        .env("TZ", "UTC")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// A home whose journal holds two requests on 2026-10-06: the first on `xai/work`, an account the
/// home doesn't have (so it is not priced: account gone), with usage; the second with no usage.
fn home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("records")).unwrap();
    let open = |id: &str, at: &str, agent: &str| json!({"v":1,"t":"open","id":id,"arrived":at,"agent":agent,"style":"anthropic-messages","op":"generate","type":"text","target":"sonnet"});
    let attempt = |id: &str| {
        json!({"v":1,"t":"attempt","id":id,"attempt":{"n":1,"provider":"xai","account":"work","model":"m","kind":"initial",
            "started":4.0,"ended":900.0,"outcome":{"state":"ok"},"dropped":[],"forced":[]}})
    };
    let close = |id: &str, usage: Value| {
        json!({"v":1,"t":"close","id":id,"outcome":"succeeded","served_by":{"provider":"xai","account":"work","model":"m"},
            "ttft_ms":120.0,"total_ms":900.0,"usage":usage,"break_handling":{"kind":"none"},"job":null})
    };
    let usage = json!({"input":1_500_000,"output":611_000,"cache_read":2_900_000,"cache_write":null,"reasoning":null,
        "input_semantics":"excludes_cache","estimated":false});
    let lines = [
        open("rq_1", "2026-10-06T10:00:00Z", "ak_a"),
        attempt("rq_1"),
        close("rq_1", usage),
        open("rq_2", "2026-10-06T11:00:00Z", "ak_b"),
        attempt("rq_2"),
        close("rq_2", Value::Null),
    ];
    let body: String = lines.iter().map(|l| l.to_string() + "\n").collect();
    std::fs::write(home.path().join("records/2026-10-06.jsonl"), body).unwrap();
    home
}

#[test]
fn the_text_follows_the_contract() {
    let h = home();
    let o = nr(h.path(), NOW, &["usage"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = out(&o);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "period: today, 2026-10-06 00:00:00 → 2026-10-06 14:02:11 UTC");
    assert_eq!(lines[1], "requests          2   (1 not reported)");
    assert_eq!(lines[2], "input (uncached)  1.5M");
    assert_eq!(lines[3], "cached            2.9M");
    assert_eq!(lines[4], "output            611k");
    assert_eq!(lines[5], "est. cost         ~$0.00   Estimated, not actual billing");
    assert_eq!(lines[6], "                  1 requests not priced: 1 account gone");
    assert_eq!(
        lines[7],
        "                  Priced with today's declared prices; earlier price changes are not tracked."
    );
    assert_eq!(lines[9], "agent             requests");
    assert_eq!(lines[10], "ak_a                     1");
    assert_eq!(lines[11], "ak_b                     1");
    assert_eq!(lines[13], "provider          requests");
    assert_eq!(lines[14], "xai                      2");
}

#[test]
fn json_is_the_view() {
    let h = home();
    let v: Value = serde_json::from_str(&out(&nr(h.path(), NOW, &["--json", "usage", "--period", "all"]))).unwrap();
    assert_eq!(v["period"], "all");
    assert_eq!(v["from"], Value::Null);
    assert_eq!(v["requests"], 2);
    assert_eq!(v["not_reported"], 1);
    assert_eq!(v["tokens"], json!({"input": 1_500_000, "cached": 2_900_000, "output": 611_000}));
    assert_eq!(v["cost"]["label"], "Estimated, not actual billing");
    assert_eq!(v["cost"]["unpriced"]["account_gone"], 1);
}

#[test]
fn an_empty_period_says_so_and_prints_no_tables() {
    let h = home();
    let o = nr(h.path(), "2026-12-01T00:00:00Z", &["usage", "--period", "24h"]);
    let text = out(&o);
    assert!(text.contains("requests          0 (no requests in this period)"), "{text}");
    assert!(!text.contains("agent "), "{text}");
}

#[test]
fn an_unknown_period_exits_one_and_names_the_six() {
    let h = home();
    let o = nr(h.path(), NOW, &["usage", "--period", "week"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr).trim(),
        "unknown period \"week\"; use today, 24h, 7d, 30d, 60d or all"
    );
}
