//! The real hermes agent against a running `nullrouter serve` and the operator's own accounts
//! (spec 004, T028 and T031). Opt-in and operator-run: it spends real quota.
//!
//! ```text
//! nullrouter keys issue hermes-live --harness hermes        # prints the key once
//! NR_LIVE=1 NR_KEY=0r-… cargo test -p nullrouter-server --test harness_hermes -- --ignored --nocapture
//! ```
//!
//! `NR_BASE` defaults to `http://127.0.0.1:20129`. `NR_MODELS` is a comma-separated list of
//! targets; the default is the six chosen text providers. A model that fails is reported with
//! hermes's output, and flagged when the failure looks like a provider refusing echoed
//! reasoning: that provider belongs in `REJECTS_ECHOED_REASONING` (T031).

use std::path::Path;
use std::process::Command;

const DEFAULT_MODELS: &[&str] = &[
    "anthropic/claude-sonnet-4-20250514",
    "openrouter/anthropic/claude-sonnet-4",
    "opencode-zen/claude-opus-5",
    "opencode-go/glm-5.3",
    "xai/grok-4",
    "grok-cli/grok-4.5",
];

/// Whether `output` reads like a provider refusing a reasoning field on an assistant message.
fn rejects_echoed_reasoning(output: &str) -> bool {
    let lower = output.to_lowercase();
    ["reasoning_content", "reasoning_details", "\"reasoning\"", "extra_forbidden"].iter().any(|f| lower.contains(f))
        && ["400", "422"].iter().any(|c| lower.contains(c))
}

#[test]
#[ignore = "live: needs NR_LIVE=1, a running server, a hermes-bound key and the operator's accounts"]
fn hermes_completes_every_turn_on_each_chosen_provider() {
    if std::env::var("NR_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set NR_LIVE=1 (and NR_KEY) to run hermes against the live providers");
        return;
    }
    let key = std::env::var("NR_KEY").expect("NR_KEY: a key issued with `--harness hermes`");
    let base = std::env::var("NR_BASE").unwrap_or_else(|_| "http://127.0.0.1:20129".into());
    let models: Vec<String> = match std::env::var("NR_MODELS") {
        Ok(m) => m.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect(),
        Err(_) => DEFAULT_MODELS.iter().map(|s| (*s).to_owned()).collect(),
    };
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/harness/hermes/run.sh");

    let (mut failed, mut reject) = (Vec::new(), Vec::new());
    for model in &models {
        let out = Command::new("bash")
            .arg(&script)
            .env("NR_BASE", &base)
            .env("NR_KEY", &key)
            .env("NR_MODEL", model)
            .output()
            .expect("bash runs the script");
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        println!("== {model}\n{text}");
        if !out.status.success() {
            if rejects_echoed_reasoning(&text) {
                reject.push(model.clone());
            }
            failed.push(model.clone());
        }
    }
    if !reject.is_empty() {
        println!("echoed reasoning looks rejected on: {reject:?}\nadd the provider ids to REJECTS_ECHOED_REASONING (T031)");
    }
    assert!(failed.is_empty(), "hermes did not complete every turn on: {failed:?}");
}
