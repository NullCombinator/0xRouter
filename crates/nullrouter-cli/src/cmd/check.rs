//! `nullrouter check`: prints the `views::check` report; exit 1 when it found errors.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::quota::fit::outside::{Alert, alerts};
use nullrouter_registry::OperatorHome;
use nullrouter_server::views;
use serde_json::{Value, json};

fn lines(v: &Value) -> impl Iterator<Item = &str> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str)
}

/// One `warn` line per unacknowledged usage alert of every account in `accounts.toml`, newest
/// first within an account (FR-026, FR-027). The id is whole, so the command can be pasted: ULIDs
/// raised minutes apart share their first characters. An unreadable accounts file: none.
fn alert_warnings(home: &Path) -> Vec<String> {
    let Ok(list) = Accounts::load(&home.join(accounts::FILE)) else { return Vec::new() };
    list.iter()
        .flat_map(|a| {
            alerts(home, &a.provider, &a.name).into_iter().map(move |alert: Alert| {
                let Alert { id, text, .. } = alert;
                let text = text.unwrap_or_else(|| format!("{}/{}: usage alert {id}", a.provider, a.name));
                format!("warn  {text} (nullrouter quota ack {id})")
            })
        })
        .collect()
}

pub(crate) fn run(home: Option<PathBuf>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let view = super::read(&home, views::check::NEEDS, &json!({}), views::check::build)?;
    let r = &view.json;
    let list = |k: &str| r[k].as_array().map_or(&[][..], Vec::as_slice);
    let s = &r["signin"];
    let modes = s["file_modes"].as_array().map_or(&[][..], Vec::as_slice);
    if as_json {
        println!("{r:#}");
    } else {
        println!("home: {}", r["home"].as_str().unwrap_or_default());
        let (url, source) = (r["endpoint"].as_str().unwrap_or_default(), r["endpoint_source"].as_str());
        if source == Some("config") {
            println!("endpoint: {url} (configured; no server running)");
        } else {
            println!("endpoint: {url}");
        }
        let (bundled, user) =
            (r["providers"]["bundled"].as_u64().unwrap_or(0), r["providers"]["user"].as_u64().unwrap_or(0));
        println!("providers: {} ({bundled} bundled, {user} user)", bundled + user);
        println!("unified models: {}", r["unified_models"]);
        for n in r["notices"].as_array().into_iter().flatten().filter_map(|n| n["text"].as_str()) {
            println!("{n}");
        }
        for w in alert_warnings(home.path()) {
            println!("{w}");
        }
    }
    let errors = !list("skipped").is_empty()
        || !list("dropped_unified_models").is_empty()
        || !list("paused_proxies").is_empty()
        || !list("dropped_combos").is_empty()
        || lines(&s["errors"]).next().is_some()
        || modes.iter().any(|m| m["error"] == true);
    Ok(ExitCode::from(u8::from(errors)))
}
