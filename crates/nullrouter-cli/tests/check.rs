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
}
