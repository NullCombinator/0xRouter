//! Claude Code through a running `nullrouter serve` with a key bound to the `claude-code` adapter
//! (spec 004, T081). Each chosen text provider gets two sessions: one that calls a tool (Bash) and
//! one that uses web search. Every record the key makes in a session must succeed and must show
//! the adapter ran. Opt-in and operator-run: it spends real quota.
//!
//! ```text
//! nullrouter keys issue cc-laptop --adapter claude-code     # prints the key once
//! NR_LIVE=1 NR_KEY=0r-… cargo test -p nullrouter-server --test harness_claude_code -- --ignored --nocapture
//! ```
//!
//! `NR_BASE` defaults to `http://127.0.0.1:20129`. `NR_MODELS` is a comma-separated list of
//! targets; the default is the six chosen text providers (as in `harness_hermes`). The operator
//! home (`NULLROUTER_HOME`, default `~/.0router`) must hold an approved, active `claude-code`
//! version (T084); without one the test skips with a message.
//!
//! `tests/harness/claude.sh` takes no prompt and always asks "Say hello.", so this test runs
//! `claude -p` itself with the same environment as that script (throwaway `CLAUDE_CONFIG_DIR`,
//! the same `ANTHROPIC_*` variables), adding the prompt and the tool allowlist each session needs.

use std::collections::HashSet;
use std::path::Path;
use std::process::{Command, Stdio};

use nullrouter_adapters::HarnessName;
use nullrouter_adapters::store::Store;
use nullrouter_engine::journal::records::{self, Filter};
use nullrouter_engine::keys::{self, Keys};
use nullrouter_registry::OperatorHome;
use serde_json::Value;

const HARNESS: &str = "claude-code";
const DEFAULT_MODELS: &[&str] = &[
    "anthropic/claude-sonnet-4-20250514",
    "openrouter/anthropic/claude-sonnet-4",
    "opencode-zen/claude-opus-5",
    "opencode-go/glm-5.3",
    "xai/grok-4",
    "grok-cli/grok-4.5",
];

/// One Claude Code session: its prompt and the only tools it may use.
struct Session {
    name: &'static str,
    prompt: &'static str,
    tools: &'static [&'static str],
    /// Text the answer must contain. `None`: any non-empty answer will do.
    expect: Option<&'static str>,
}

const SESSIONS: &[Session] = &[
    Session {
        name: "tool call",
        prompt: "Run `echo nr-tool-7f3` with the Bash tool, then reply with only its output.",
        tools: &["Bash"],
        expect: Some("nr-tool-7f3"),
    },
    Session {
        name: "web search",
        prompt: "Use the WebSearch tool to find the official Rust programming language website, then reply with one line naming it.",
        tools: &["WebSearch"],
        expect: None,
    },
];

/// The reason the `claude-code` adapter cannot serve from `home`, or `None` when it has an
/// approved active version.
fn not_serving(home: &Path) -> Option<String> {
    if !home.join("adapters").is_dir() {
        return Some(format!(
            "no adapters under {}: install and approve {HARNESS} first (quickstart § 10)",
            home.display()
        ));
    }
    let store = match Store::open(home) {
        Ok(s) => s,
        Err(e) => return Some(format!("the adapter store: {e}")),
    };
    let index = match store.load_index() {
        Ok(i) => i,
        Err(e) => return Some(format!("the adapter index: {e}")),
    };
    let name = HarnessName::new(HARNESS).expect("a valid harness name");
    index.serving(&name).is_none().then(|| format!("{HARNESS} has no approved active version in {}", home.display()))
}

/// The id of the key `key` in `home`, which must be bound to the `claude-code` adapter.
fn agent_id(home: &Path, key: &str) -> String {
    let keys = Keys::load(&home.join(keys::FILE)).expect("keys.toml in the operator home");
    let found = keys.lookup(key).unwrap_or_else(|| panic!("NR_KEY is not a key in {}", home.display()));
    assert_eq!(
        found.adapter.as_ref().map(HarnessName::as_str),
        Some(HARNESS),
        "the key must be issued with --adapter {HARNESS}"
    );
    found.id.clone()
}

/// Every record the key `key_id` has made, newest first.
fn key_records(home: &Path, key_id: &str) -> Vec<Value> {
    records::read(home, &Filter { agent: Some(key_id.to_owned()), ..Filter::default() })
}

/// Whether some attempt of `r` ran the `claude-code` adapter.
fn adapter_ran(r: &Value) -> bool {
    r["attempts"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|a| a["adapter"]["harness"] == HARNESS && a["adapter"]["outcome"]["state"] == "ran")
}

/// Claude Code in print mode, as `tests/harness/claude.sh` runs it, with this session's prompt
/// and tools. Returns its output and whether it exited successfully.
fn run_claude(base: &str, key: &str, model: &str, session: &Session) -> (String, bool) {
    let cfg = tempfile::tempdir().expect("a throwaway config directory");
    let out = Command::new("timeout")
        .args(["180", "claude", "-p", session.prompt, "--model", model, "--allowedTools"])
        .args(session.tools)
        .env("CLAUDE_CONFIG_DIR", cfg.path())
        .env("ANTHROPIC_BASE_URL", base)
        .env("ANTHROPIC_API_KEY", key)
        .env("ANTHROPIC_MODEL", model)
        .env("ANTHROPIC_SMALL_FAST_MODEL", model)
        .env("ANTHROPIC_DEFAULT_HAIKU_MODEL", model)
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_AUTOUPDATER", "1")
        .current_dir(cfg.path())
        .stdin(Stdio::null())
        .output()
        .expect("timeout runs claude");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (text, out.status.success())
}

#[test]
#[ignore = "live: needs NR_LIVE=1, a running server, a claude-code key and an approved claude-code adapter"]
fn claude_code_sessions_use_tools_and_web_search_on_each_chosen_provider() {
    if std::env::var("NR_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set NR_LIVE=1 (and NR_KEY) to run Claude Code against the live providers");
        return;
    }
    let home = OperatorHome::resolve();
    if let Some(why) = not_serving(home.path()) {
        eprintln!("skipped: {why}");
        return;
    }
    let key = std::env::var("NR_KEY").expect("NR_KEY: a key issued with `--adapter claude-code`");
    let base = std::env::var("NR_BASE").unwrap_or_else(|_| "http://127.0.0.1:20129".into());
    let models: Vec<String> = match std::env::var("NR_MODELS") {
        Ok(m) => m.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect(),
        Err(_) => DEFAULT_MODELS.iter().map(|s| (*s).to_owned()).collect(),
    };
    let key_id = agent_id(home.path(), &key);

    let mut problems = Vec::new();
    for model in &models {
        for session in SESSIONS {
            let label = format!("{model} / {}", session.name);
            let before: HashSet<String> =
                key_records(home.path(), &key_id).iter().filter_map(|r| r["id"].as_str().map(str::to_owned)).collect();

            let (text, ok) = run_claude(&base, &key, model, session);
            println!("== {label}\n{text}");
            if !ok {
                problems.push(format!("{label}: claude exited with an error"));
            }
            if let Some(want) = session.expect
                && !text.contains(want)
            {
                problems.push(format!("{label}: the answer lacks {want:?}"));
            }

            let fresh: Vec<Value> = key_records(home.path(), &key_id)
                .into_iter()
                .filter(|r| r["id"].as_str().is_some_and(|id| !before.contains(id)))
                .collect();
            if fresh.is_empty() {
                problems.push(format!("{label}: no record reached the server for this key"));
            }
            for r in &fresh {
                let id = r["id"].as_str().unwrap_or("?");
                if r["outcome"] != "succeeded" {
                    problems.push(format!("{label}: record {id} is {}", r["outcome"]));
                }
                if !adapter_ran(r) {
                    problems.push(format!("{label}: record {id} did not run {HARNESS}: {}", r["attempts"]));
                }
            }
        }
    }
    assert!(problems.is_empty(), "Claude Code did not pass on every provider and session:\n{}", problems.join("\n"));
}
