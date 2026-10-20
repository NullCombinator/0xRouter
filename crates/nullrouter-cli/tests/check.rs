//! `nullrouter check` on the sign-in files (T092): token entries without a sign-in account,
//! sign-in accounts without tokens, and file modes. A token is never printed.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use nullrouter_engine::files::write_private;

const TOKEN: &str = "at-SENTINEL-T092";
const REFRESH: &str = "rt-SENTINEL-T092";

fn nr(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nullrouter")).arg("--home").arg(home).args(args).output().unwrap()
}

fn chmod(p: &Path, mode: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

fn token(provider: &str, name: &str) -> String {
    format!(
        "[[token]]\nprovider = \"{provider}\"\nname = \"{name}\"\naccess_token = \"{TOKEN}\"\n\
         refresh_token = \"{REFRESH}\"\nexpires_at = \"2030-01-01T00:00:00Z\"\n\
         hosts = [\"api.x.ai\"]\nsigned_in_at = \"2026-10-01T00:00:00Z\"\n"
    )
}

/// accounts: grok-cli/work (sign-in, has tokens), grok-cli/spare (sign-in, no tokens),
/// anthropic/main (key). tokens: grok-cli/work, grok-cli/gone (no account), anthropic/main
/// (a key account).
fn home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    write_private(
        &h.join("accounts.toml"),
        "schema = 2\n\
         [[account]]\nprovider = \"grok-cli\"\nname = \"work\"\nkind = \"signin\"\n\
         [[account]]\nprovider = \"grok-cli\"\nname = \"spare\"\nkind = \"signin\"\n\
         [[account]]\nprovider = \"anthropic\"\nname = \"main\"\nkind = \"key\"\nsecret = \"sk-x\"\n",
    )
    .unwrap();
    let tokens =
        format!("schema = 1\n{}{}{}", token("grok-cli", "work"), token("grok-cli", "gone"), token("anthropic", "main"));
    write_private(&h.join("tokens.toml"), &tokens).unwrap();
    dir
}

fn assert_no_token(text: &str) {
    assert!(!text.contains(TOKEN) && !text.contains(REFRESH), "a token was printed:\n{text}");
}

#[test]
fn reports_unmatched_tokens_and_tokenless_accounts() {
    let dir = home();
    let out = nr(dir.path(), &["check"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success(), "warnings only: {text}");
    assert_no_token(&text);
    assert!(text.contains("tokens for grok-cli/gone, which is not a sign-in account"), "{text}");
    assert!(text.contains("tokens for anthropic/main, which is not a sign-in account"), "{text}");
    assert!(!text.contains("grok-cli/work"), "a matched account is not reported: {text}");
    assert!(
        text.contains("sign-in account grok-cli/spare has no tokens and can't serve; run `nullrouter accounts signin grok-cli spare`"),
        "{text}"
    );

    let out = nr(dir.path(), &["--json", "check"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_no_token(&text);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let s = &v["signin"];
    assert_eq!(s["tokens_without_account"].as_array().unwrap().len(), 2, "{s}");
    assert_eq!(s["accounts_without_tokens"][0]["name"], "spare");
    assert_eq!(s["accounts_without_tokens"][0]["fix"], "nullrouter accounts signin grok-cli spare");
    assert!(s["errors"].as_array().unwrap().is_empty() && s["file_modes"].as_array().unwrap().is_empty(), "{s}");
}

#[test]
fn loose_modes_are_warnings_and_a_shared_tokens_file_is_an_error() {
    let dir = home();
    let h = dir.path();
    write_private(&h.join("install-id"), "00000000-0000-4000-8000-000000000000\n").unwrap();
    chmod(&h.join("install-id"), 0o644);
    fs::write(h.join("tokens.lock"), "").unwrap();
    chmod(&h.join("tokens.lock"), 0o640);
    let q = h.join("quota/grok-cli");
    fs::create_dir_all(&q).unwrap();
    chmod(&h.join("quota"), 0o755);
    chmod(&q, 0o700);
    fs::write(q.join("work.jsonl"), "").unwrap();
    chmod(&q.join("work.jsonl"), 0o644);
    fs::write(q.join("work.tally.json"), "{}").unwrap();
    chmod(&q.join("work.tally.json"), 0o600);

    let out = nr(h, &["check"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success(), "loose modes outside tokens.toml are warnings: {text}");
    for (path, want) in [("install-id", "0600"), ("tokens.lock", "0600"), ("quota has", "0700"), ("work.jsonl", "0600")]
    {
        assert!(
            text.lines()
                .any(|l| l.starts_with("warning: ") && l.contains(path) && l.contains(&format!("expected {want}"))),
            "{path}: {text}"
        );
    }
    assert!(!text.contains("tally.json") && !text.contains("quota/grok-cli has"), "private paths pass: {text}");

    chmod(&h.join("tokens.toml"), 0o644);
    let out = nr(h, &["check"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert_no_token(&text);
    assert!(
        text.lines().any(|l| l.starts_with("error: ")
            && l.contains("tokens.toml has mode 644")
            && l.contains("refuses to start")),
        "{text}"
    );
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8(nr(h, &["--json", "check"]).stdout).unwrap()).unwrap();
    let modes = v["signin"]["file_modes"].as_array().unwrap();
    assert!(modes.iter().any(|m| m["error"] == true && m["mode"] == "644"), "{modes:?}");
}

#[test]
fn a_malformed_tokens_file_is_an_error_without_its_source_line() {
    let dir = home();
    let h = dir.path();
    write_private(&h.join("tokens.toml"), &format!("schema = 1\n[[token]]\naccess_token = \"{TOKEN}\" junk\n"))
        .unwrap();
    let out = nr(h, &["check"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert_no_token(&text);
    assert!(text.lines().any(|l| l.starts_with("error: ") && l.contains("tokens.toml")), "{text}");
    // L4: every command that reads the file names the place, never the line.
    for args in [
        &["accounts", "list"][..],
        &["accounts", "remove", "grok-cli", "work"],
        &["accounts", "enable", "grok-cli", "work"],
    ] {
        let out = nr(h, args);
        let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert_no_token(&all);
    }
    let out = nr(h, &["accounts", "list"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("tokens.toml: line 3, column "), "{err}");
}

#[test]
fn reported_windows_with_no_meter_are_noted_and_paced_in_their_own_unit() {
    let dir = tempfile::tempdir().unwrap();
    let text = String::from_utf8(nr(dir.path(), &["check"]).stdout).unwrap();
    assert!(
        text.contains("note: grok-cli reports window prepaid, which no [[routing.window]] meter names"),
        "a credit balance has no length to declare:\n{text}"
    );
    let json: serde_json::Value = serde_json::from_slice(&nr(dir.path(), &["--json", "check"]).stdout).unwrap();
    let listed = json["unmetered_windows"].as_array().unwrap();
    assert!(listed.iter().any(|w| w["provider"] == "grok-cli" && w["window"] == "prepaid"), "{listed:?}");
}

/// A home with a user plugin and a unified model whose members declare different limits.
fn mixed_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    fs::create_dir(h.join("plugins")).unwrap();
    let blue = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/community/bluesminds.toml");
    fs::copy(blue, h.join("plugins/bluesminds.toml")).unwrap();
    fs::write(
        h.join("config.toml"),
        "schema = 1\n[[unified_model]]\nname = \"mixed\"\nmembers = [\n\
         { provider = \"grok-cli\", model = \"grok-build\" },\n\
         { provider = \"bluesminds\", model = \"claude-sonnet-4-5\" },\n]\n",
    )
    .unwrap();
    dir
}

const NOTE: &str = "note: unified model mixed: members differ in context_length: grok-cli 500000, bluesminds 200000";

#[test]
fn check_and_resolve_note_members_with_different_limits() {
    let dir = mixed_home();
    let h = dir.path();
    let out = nr(h, &["check"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(NOTE), "{text}");
    assert!(text.contains("members differ in max_output_tokens: grok-cli 64000, bluesminds undeclared"), "{text}");
    // A note is not a failure: the model loads and the check passes.
    assert_eq!(out.status.code(), Some(0), "{text}");

    let out = nr(h, &["resolve", "mixed"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("unified mixed:"), "{text}");
    assert!(text.contains(NOTE), "{text}");
    assert_eq!(out.status.code(), Some(0));

    let json: serde_json::Value = serde_json::from_slice(&nr(h, &["--json", "check"]).stdout).unwrap();
    let notes = json["limits_notes"].as_array().unwrap();
    assert_eq!(notes[0]["unified"], "mixed");
    assert_eq!(notes[0]["limit"], "context_length");
    assert_eq!(notes[0]["values"][1]["provider"], "bluesminds");
    assert_eq!(notes[0]["values"][1]["value"], 200000);
    let json: serde_json::Value = serde_json::from_slice(&nr(h, &["--json", "resolve", "mixed"]).stdout).unwrap();
    assert_eq!(json["limits_notes"].as_array().unwrap().len(), 2);

    // Another target's resolve says nothing about it.
    let text = String::from_utf8(nr(h, &["resolve", "grok-cli/grok-build"]).stdout).unwrap();
    assert!(!text.contains("differ"), "{text}");
}

/// A user plugin with no quota, meter or price, whose only endpoint speaks Gemini, declaring
/// cache mode `explicit` (T093).
const MUTE: &str = r#"schema = 2
id = "mute"
category = "apikey"

[auth]
kind = "apikey"
header = "x-goog-api-key"

[endpoints.text]
url = "https://mute.example/v1beta/models/{model}:generateContent"
wire = "gemini"

[[models]]
id = "m1"

[routing.cache]
mode = "explicit"
"#;

#[test]
fn routing_declarations_that_cant_work_are_warned() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    fs::create_dir(h.join("plugins")).unwrap();
    fs::write(h.join("plugins/mute.toml"), MUTE).unwrap();
    write_private(
        &h.join("accounts.toml"),
        "schema = 2\n[[account]]\nprovider = \"mute\"\nname = \"a\"\nsecret = \"k\"\n\
         [[account]]\nprovider = \"mute\"\nname = \"priced\"\nsecret = \"k\"\n\
         [account.routing]\nprice = { input = 1.0 }\n",
    )
    .unwrap();
    fs::create_dir_all(h.join("routing")).unwrap();
    fs::write(h.join("routing/salt"), [0u8; 32]).unwrap();
    chmod(&h.join("routing/salt"), 0o644);
    chmod(&h.join("routing"), 0o755);

    let out = nr(h, &["check"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "warnings only: {text}");
    assert!(text.contains("warning: mute/a is pay-as-you-go with no price in its plugin or account"), "{text}");
    assert!(!text.contains("mute/priced"), "an account price is a price: {text}");
    assert!(
        text.contains(
            "warning: mute declares cache mode explicit, but none of its endpoints speaks a style with cache markers"
        ),
        "{text}"
    );
    assert!(text.contains("routing/salt has mode 644, expected 0600"), "{text}");
    assert!(text.lines().any(|l| l.contains("/routing has mode 755, expected 0700")), "{text}");

    let json: serde_json::Value = serde_json::from_slice(&nr(h, &["--json", "check"]).stdout).unwrap();
    assert_eq!(json["routing_warnings"].as_array().unwrap().len(), 2, "{json:#}");
}

#[test]
fn bundled_plugins_raise_no_routing_warning() {
    let dir = tempfile::tempdir().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&nr(dir.path(), &["--json", "check"]).stdout).unwrap();
    assert_eq!(json["routing_warnings"], serde_json::json!([]));
}

/// T057: an unacknowledged usage alert is a `warn` line naming its whole id, and the exit
/// status is 0 with or without it. Alerts are read per account in `accounts.toml`.
#[test]
fn unacknowledged_alerts_are_warnings_and_leave_the_exit_status_alone() {
    const ENTRY: &str = "01JB7AAAAAAAAAAAAAAAAAAAAA";
    const ALERT: &str = "01JB8AAAAAAAAAAAAAAAAAAAAA";
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    write_private(
        &h.join("accounts.toml"),
        "schema = 2\n[[account]]\nprovider = \"opencode-go\"\nname = \"main\"\nsecret = \"sk-go-SENTINEL-T057\"\n",
    )
    .unwrap();
    let out = nr(h, &["check"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(!text.contains("warn  "), "no alert, no warning: {text}");

    let entry = format!(
        r#"{{"v":1,"kind":"entry","id":"{ENTRY}","window":"weekly","type":"idle","start":"2026-10-07T02:10:00.000Z","end":"2026-10-07T02:30:00.000Z","amount":4.0,"unit":"percent","found_at":"2026-10-07T02:31:00.000Z"}}"#
    );
    let alert = format!(r#"{{"v":1,"kind":"alert","id":"{ALERT}","entry":"{ENTRY}","raised_at":"2026-10-07T02:31:00.000Z"}}"#);
    let q = h.join("quota/opencode-go");
    fs::create_dir_all(&q).unwrap();
    chmod(&h.join("quota"), 0o700);
    chmod(&q, 0o700);
    write_private(&q.join("main.outside.jsonl"), &format!("{entry}\n{alert}\n")).unwrap();

    let out = nr(h, &["check"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "warnings only: {text}");
    assert!(
        text.contains(&format!("warn  opencode-go/main: usage alert {ALERT} (nullrouter quota ack {ALERT})")),
        "{text}"
    );

    // A line with text shows that text (T059).
    const TEXTED: &str = "01JB9AAAAAAAAAAAAAAAAAAAAA";
    const WORDS: &str = "opencode-go/main: 4% of weekly used 02:10\u{2013}02:30 Wed with no traffic from 0router";
    let texted = format!(
        r#"{{"v":1,"kind":"alert","id":"{TEXTED}","entry":"{ENTRY}","raised_at":"2026-10-07T02:32:00.000Z","text":"{WORDS}"}}"#
    );
    write_private(&q.join("main.outside.jsonl"), &format!("{entry}\n{alert}\n{texted}\n")).unwrap();
    let out = nr(h, &["check"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "warnings only: {text}");
    assert!(text.contains(&format!("warn  {WORDS} (nullrouter quota ack {TEXTED})")), "{text}");
    assert!(text.contains(&format!("usage alert {ALERT}")), "old line keeps the fallback: {text}");
    assert_no_token(&text);
}
