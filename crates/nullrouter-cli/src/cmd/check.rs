//! `nullrouter check`: prints the `views::check` report; exit 1 when it found errors.

use std::path::PathBuf;
use std::process::ExitCode;

use nullrouter_registry::OperatorHome;
use nullrouter_server::views;
use serde_json::{Value, json};

fn lines<'a>(v: &'a Value) -> impl Iterator<Item = &'a str> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str)
}

fn pair(v: &Value) -> (&str, &str) {
    (v["provider"].as_str().unwrap_or_default(), v["name"].as_str().unwrap_or_default())
}

pub(crate) fn run(home: Option<PathBuf>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let view = super::read(&home, views::check::NEEDS, &json!({}), views::check::build)?;
    let (r, x) = (&view.json, &view.extra);
    let list = |k: &str| r[k].as_array().map_or(&[][..], Vec::as_slice);
    let s = &r["signin"];
    let modes = s["file_modes"].as_array().map_or(&[][..], Vec::as_slice);
    if as_json {
        println!("{r:#}");
    } else {
        println!("home: {}", r["home"].as_str().unwrap_or_default());
        let (bundled, user) =
            (r["providers"]["bundled"].as_u64().unwrap_or(0), r["providers"]["user"].as_u64().unwrap_or(0));
        println!("providers: {} ({bundled} bundled, {user} user)", bundled + user);
        println!("unified models: {}", r["unified_models"]);
        for c in list("pending_conflicts") {
            let (id, path) = (c["id"].as_str().unwrap_or_default(), c["path"].as_str().unwrap_or_default());
            println!(
                "conflict pending: {path} shadows bundled {id}; bundled is active until plugin_decisions.{id} is set"
            );
        }
        for c in list("declined") {
            let (id, path) = (c["id"].as_str().unwrap_or_default(), c["path"].as_str().unwrap_or_default());
            println!("declined: {path} (bundled {id} stays active)");
        }
        for w in lines(&x["withheld"]) {
            println!("credential WITHHELD: {w}");
        }
        for sk in list("skipped") {
            println!("skipped: {}", sk["path"].as_str().unwrap_or_default());
            for e in lines(&sk["errors"]) {
                println!("  {e}");
            }
        }
        for d in list("dropped_unified_models") {
            println!(
                "dropped unified model {}: member provider {} was skipped",
                d["name"].as_str().unwrap_or_default(),
                d["provider"].as_str().unwrap_or_default()
            );
        }
        for n in lines(&x["notes"]) {
            println!("note: {n}");
        }
        let j = &r["journal"];
        if j["kept"] == false {
            println!(
                "warning: records not kept since {} (disk full): {} requests",
                j["since"].as_str().unwrap_or("?"),
                j["unkept_requests"]
            );
        }
        for u in list("unmetered_windows") {
            println!(
                "note: {} reports window {}, which no [[routing.window]] meter names; it is paced in its own unit",
                u["provider"].as_str().unwrap_or_default(),
                u["window"].as_str().unwrap_or_default()
            );
        }
        for w in lines(&r["routing_warnings"]) {
            println!("warning: {w}");
        }
        for e in lines(&s["errors"]) {
            println!("error: {e}");
        }
        for (m, line) in modes.iter().zip(lines(&x["mode_lines"])) {
            println!("{}: {line}", if m["error"] == true { "error" } else { "warning" });
        }
        for t in s["tokens_without_account"].as_array().into_iter().flatten() {
            let (p, n) = pair(t);
            println!("warning: tokens.toml has tokens for {p}/{n}, which is not a sign-in account; they are ignored");
        }
        for a in s["accounts_without_tokens"].as_array().into_iter().flatten() {
            let (p, n) = pair(a);
            println!(
                "warning: sign-in account {p}/{n} has no tokens and can't serve; run `{}`",
                a["fix"].as_str().unwrap_or_default()
            );
        }
    }
    let errors = !list("skipped").is_empty()
        || !list("dropped_unified_models").is_empty()
        || lines(&s["errors"]).next().is_some()
        || modes.iter().any(|m| m["error"] == true);
    Ok(ExitCode::from(u8::from(errors)))
}
