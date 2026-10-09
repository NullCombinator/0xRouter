//! Adapter output through the binary (T085): `records show` text and JSON for an adapter that ran
//! with changes, one that ran with none, and a blocked one; `adapters list`; `alerts list` and
//! `alerts ack`. Homes are written by hand in the on-disk formats the engine and the adapter store
//! read, so these tests do not depend on a running server.

use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

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

/// A fingerprint in the form `SourceFp` parses: `sha256:` and 64 lowercase hex characters.
fn fp(c: char) -> String {
    format!("sha256:{}", c.to_string().repeat(64))
}

/// Appends record lines for `day` to the journal the way the engine lays it out.
fn journal(home: &Path, day: &str, lines: &[serde_json::Value]) {
    std::fs::create_dir_all(home.join("records")).unwrap();
    let path = home.join(format!("records/{day}.jsonl"));
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(path, format!("{old}{text}")).unwrap();
}

/// One record with three attempts: the first ran an adapter with three changes, the second ran
/// one with none, the third was blocked by a guardrail. The response adapter is the first run's.
fn three_adapter_records(home: &Path) {
    let id = "rq_01";
    let hermes = serde_json::json!({
        "harness": "hermes", "version": "builtin", "outcome": {"state": "ran"}, "duration_us": 12,
        "changes": [
            {"path": "messages[4].images", "kind": "converted", "reason": "format_conversion"},
            {"path": "messages[4].content", "kind": "converted", "reason": "format_conversion"},
            {"path": "messages[3].reasoning_content", "kind": "removed", "reason": "target_rejects_field"}
        ]
    });
    let claude_none = serde_json::json!({
        "harness": "claude-code", "version": "v0.1.0-1a2b3c4d", "outcome": {"state": "ran"},
        "changes": [], "duration_us": 9
    });
    let claude_blocked = serde_json::json!({
        "harness": "claude-code", "version": "v0.1.0-1a2b3c4d", "outcome": {"state": "blocked"},
        "changes": [], "duration_us": 9,
        "guardrail": {"direction": "request", "rule": "tool_call_added", "paths": ["messages[7].content[2]"],
            "adapter": {"harness": "claude-code", "version": "v0.1.0-1a2b3c4d"}, "at": "2026-09-28T11:04:52Z"}
    });
    let attempt = |n: u64, provider: &str, model: &str, outcome: serde_json::Value, adapter: serde_json::Value| {
        serde_json::json!({"v": 1, "t": "attempt", "id": id, "attempt": {
            "n": n, "provider": provider, "account": "main", "model": model, "kind": "initial",
            "started": 0.4, "ended": 900.0, "outcome": outcome, "dropped": [], "forced": [],
            "adapter": adapter}})
    };
    journal(
        home,
        "2026-09-28",
        &[
            serde_json::json!({"v": 1, "t": "open", "id": id, "arrived": "2026-09-28T11:04:52.250Z",
                "agent": "hermes-desktop", "session": "44ab",
                "style": "openai-chat", "op": "generate", "type": "text", "target": "deepseek-r1"}),
            attempt(1, "openrouter", "deepseek/deepseek-r1", serde_json::json!({"state": "ok"}), hermes),
            attempt(2, "anthropic", "claude-sonnet-4-5", serde_json::json!({"state": "ok"}), claude_none),
            attempt(
                3,
                "anthropic",
                "claude-sonnet-4-5",
                serde_json::json!({"state": "failed", "status": null, "class": "transient", "reason": "overloaded"}),
                claude_blocked,
            ),
            serde_json::json!({"v": 1, "t": "close", "id": id, "outcome": "succeeded",
                "served_by": {"provider": "openrouter", "account": "main", "model": "deepseek/deepseek-r1"},
                "ttft_ms": 120.0, "total_ms": 900.0, "usage": null, "break_handling": {"kind": "none"},
                "job": null, "response_adapter": {"harness": "hermes", "version": "builtin",
                    "outcome": {"state": "ran"}, "duration_us": 12,
                    "changes": [{"path": "messages[4].images", "kind": "converted", "reason": "format_conversion"}]}}),
        ],
    );
}

/// `records show rq_01` in text lines, for the assertions below.
fn show_text(home: &Path) -> String {
    let out = nr(home, &["records", "show", "rq_01"], "");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn records_show_prints_the_adapter_runs_with_changes_none_and_blocked() {
    let dir = tempfile::tempdir().unwrap();
    three_adapter_records(dir.path());
    let text = show_text(dir.path());

    // Header and the attempt header, as the contract lays them out.
    assert!(text.starts_with("rq_01  2026-09-28 11:04:52  succeeded\n"), "{text}");
    assert!(text.contains("\n  1  openrouter/main"), "{text}");

    // The agent line names the harness of the first adapter run (the hermes built-in).
    assert!(text.contains("agent       hermes-desktop / session 44ab   harness hermes (built-in)\n"), "{text}");

    // A run with changes: the adapter line, then one line per change at the contract's indent.
    for want in [
        "     adapter hermes built-in: ran, 3 changes\n",
        "       converted  messages[4].images         format_conversion\n",
        "       converted  messages[4].content        format_conversion\n",
        "       removed    messages[3].reasoning_content  target_rejects_field\n",
    ] {
        assert!(text.contains(want), "{want:?} missing from:\n{text}");
    }

    // A run with no changes still names the adapter and its version (US6-2).
    assert!(text.contains("     adapter claude-code v0.1.0-1a2b3c4d: ran, 0 changes\n"), "{text}");

    // A blocked run: the reason, the rule and paths, and the unmodified send.
    for want in [
        "     adapter claude-code v0.1.0-1a2b3c4d: blocked by guardrail\n",
        "       rule tool_call_added  paths messages[7].content[2]\n",
        "       sent unmodified; adapter marked suspect\n",
    ] {
        assert!(text.contains(want), "{want:?} missing from:\n{text}");
    }

    // The response adapter's line follows the attempts.
    assert!(text.contains("response adapter: hermes built-in: ran, 1 change\n"), "{text}");
}

#[test]
fn records_show_json_carries_the_adapter_run_fields() {
    let dir = tempfile::tempdir().unwrap();
    three_adapter_records(dir.path());
    let out = nr(dir.path(), &["--json", "records", "show", "rq_01"], "");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    let first = &r["attempts"][0]["adapter"];
    assert_eq!(first["harness"], "hermes");
    assert_eq!(first["version"], "builtin");
    assert_eq!(first["outcome"]["state"], "ran");
    assert_eq!(first["duration_us"], 12);
    assert_eq!(first["changes"][2]["path"], "messages[3].reasoning_content");
    assert_eq!(first["changes"][2]["kind"], "removed");
    assert_eq!(first["changes"][2]["reason"], "target_rejects_field");
    assert!(first.get("guardrail").is_none_or(serde_json::Value::is_null));

    let none = &r["attempts"][1]["adapter"];
    assert_eq!(none["version"], "v0.1.0-1a2b3c4d");
    assert_eq!(none["changes"].as_array().map(Vec::len), Some(0));

    let blocked = &r["attempts"][2]["adapter"];
    assert_eq!(blocked["outcome"]["state"], "blocked");
    assert_eq!(blocked["guardrail"]["rule"], "tool_call_added");
    assert_eq!(blocked["guardrail"]["paths"][0], "messages[7].content[2]");
    assert_eq!(blocked["guardrail"]["adapter"]["harness"], "claude-code");

    assert_eq!(r["response_adapter"]["harness"], "hermes");
    assert_eq!(r["response_adapter"]["outcome"]["state"], "ran");
}

/// With the alert log present, the guardrail line names the alert the block raised on this record
/// (`al_0000000001` in ALERTS, a guardrail alert for rq_01 from the same harness and version).
#[test]
fn records_show_names_the_guardrail_alert_for_the_block() {
    let dir = tempfile::tempdir().unwrap();
    three_adapter_records(dir.path());
    adapters_home(dir.path(), INDEX, ALERTS);
    let text = show_text(dir.path());
    assert!(text.contains("       sent unmodified; adapter marked suspect (alert al_0000000001)\n"), "{text}");
    // The adapter-failed alert for rq_02 is another record's and is not named.
    assert!(!text.contains("al_0000000003"), "{text}");
}

/// A request with no adapter run: the agent line has no harness part, and the response adapter is none.
#[test]
fn records_show_without_an_adapter_has_no_harness_part() {
    let dir = tempfile::tempdir().unwrap();
    let id = "rq_01";
    journal(
        dir.path(),
        "2026-09-28",
        &[
            serde_json::json!({"v": 1, "t": "open", "id": id, "arrived": "2026-09-28T11:04:52.250Z",
                "agent": "hermes-desktop", "session": "44ab",
                "style": "openai-chat", "op": "generate", "type": "text", "target": "deepseek-r1"}),
            serde_json::json!({"v": 1, "t": "attempt", "id": id, "attempt": {
                "n": 1, "provider": "openrouter", "account": "main", "model": "deepseek/deepseek-r1", "kind": "initial",
                "started": 0.4, "ended": 900.0, "outcome": {"state": "ok"}, "dropped": [], "forced": []}}),
            serde_json::json!({"v": 1, "t": "close", "id": id, "outcome": "succeeded",
                "served_by": {"provider": "openrouter", "account": "main", "model": "deepseek/deepseek-r1"},
                "ttft_ms": 120.0, "total_ms": 900.0, "usage": null, "break_handling": {"kind": "none"}, "job": null}),
        ],
    );
    let text = show_text(dir.path());
    assert!(text.contains("agent       hermes-desktop / session 44ab\n"), "{text}");
    assert!(!text.contains("harness"), "{text}");
    assert!(!text.contains("adapter hermes") && !text.contains("adapter claude"), "{text}");
    assert!(text.contains("response adapter: none\n"), "{text}");
}

/// Writes `adapters/` (mode 0700) with `index.toml` and `alerts.toml` as the store reads them.
fn adapters_home(home: &Path, index: &str, alerts: &str) {
    std::fs::DirBuilder::new().mode(0o700).create(home.join("adapters")).unwrap();
    std::fs::write(home.join("adapters/index.toml"), index).unwrap();
    std::fs::write(home.join("adapters/alerts.toml"), alerts).unwrap();
}

const INDEX: &str = r#"schema = 1

[[harness]]
name = "claude-code"
active = "v0.1.0-1a2b3c4d"
source = "local"

[[harness.version]]
id = "v0.1.0-1a2b3c4d"
semver = "0.1.0"
state = "approved"
source_fp = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
submitted = "2026-09-20T10:00:00Z"
rebuilding = true
origin = { local = "kit-upgrade" }

[[harness.version]]
id = "v0.0.9-deadbeef"
semver = "0.0.9"
state = "superseded"
source_fp = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
submitted = "2026-09-01T10:00:00Z"
origin = { local = "kit-upgrade" }
"#;

const ALERTS: &str = r#"[[alert]]
id = "al_0000000001"
kind = "guardrail"
harness = "claude-code"
version = "v0.1.0-1a2b3c4d"
record = "rq_01"
detail = "guardrail blocked a request: tool_call_added"
at = "2026-09-28T11:04:52Z"

[[alert]]
id = "al_0000000002"
kind = "adapter_failed"
harness = "claude-code"
version = "v0.1.0-1a2b3c4d"
detail = "adapter failed: deadline"
at = "2026-09-29T09:30:00Z"
acked = "2026-09-29T10:00:00Z"

[[alert]]
id = "al_0000000003"
kind = "adapter_failed"
harness = "claude-code"
version = "v0.1.0-1a2b3c4d"
record = "rq_02"
detail = "adapter failed: trap"
at = "2026-09-30T08:00:00Z"
"#;

#[test]
fn adapters_list_shows_states_versions_and_open_alert_counts() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    adapters_home(h, INDEX, ALERTS);

    let text = String::from_utf8(nr(h, &["adapters", "list"], "").stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "hermes  built-in", "{text}");
    // Name, active version, its state, and the unacknowledged alerts (two; the third is acked).
    assert_eq!(lines[1], "claude-code  v0.1.0-1a2b3c4d  approved  2 alerts", "{text}");
    assert_eq!(lines[2], "  v0.1.0-1a2b3c4d  0.1.0  approved rebuilding", "{text}");
    assert_eq!(lines[3], "  v0.0.9-deadbeef  0.0.9  superseded", "{text}");

    let out = nr(h, &["--json", "adapters", "list"], "");
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(rows[0]["harness"], "hermes");
    assert_eq!(rows[0]["built_in"], true);
    assert_eq!(rows[1]["harness"], "claude-code");
    assert_eq!(rows[1]["built_in"], false);
    assert_eq!(rows[1]["active"], "v0.1.0-1a2b3c4d");
    assert_eq!(rows[1]["alerts"], 2);
    assert_eq!(rows[1]["versions"][0]["state"], "approved");
    assert_eq!(rows[1]["versions"][0]["rebuilding"], true);
    assert_eq!(rows[1]["versions"][1]["state"], "superseded");
}

#[test]
fn alerts_list_ack_and_ack_all_follow_the_contract() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    adapters_home(h, INDEX, ALERTS);

    // Unacknowledged only, newest first: 3 then 1.
    let text = String::from_utf8(nr(h, &["alerts", "list"], "").stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[0].starts_with("al_0000000003  adapter_failed  claude-code@v0.1.0-1a2b3c4d  rq_02"), "{text}");
    assert!(lines[1].starts_with("al_0000000001  guardrail  claude-code@v0.1.0-1a2b3c4d  rq_01"), "{text}");

    // --all includes the acknowledged one.
    let all = String::from_utf8(nr(h, &["alerts", "list", "--all"], "").stdout).unwrap();
    assert_eq!(all.lines().count(), 3, "{all}");
    assert!(all.lines().any(|l| l.starts_with("al_0000000002")), "{all}");

    // Acknowledge one by id.
    let out = nr(h, &["alerts", "ack", "al_0000000003"], "");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "acknowledged al_0000000003");
    let text = String::from_utf8(nr(h, &["alerts", "list"], "").stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.starts_with("al_0000000001"), "{text}");

    // Acknowledging it again, or an unknown id, is an error with exit 1.
    let out = nr(h, &["alerts", "ack", "al_0000000003"], "");
    assert_eq!(out.status.code(), Some(1));
    let out = nr(h, &["alerts", "ack", "al_9999999999"], "");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no open alert al_9999999999"));

    // --all acknowledges what is left: one.
    let out = nr(h, &["alerts", "ack", "--all"], "");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "acknowledged 1");
    let text = String::from_utf8(nr(h, &["alerts", "list"], "").stdout).unwrap();
    assert!(text.trim().is_empty(), "{text}");

    // Neither an id nor --all is a usage error with exit 2.
    let out = nr(h, &["alerts", "ack"], "");
    assert_eq!(out.status.code(), Some(2));
}
