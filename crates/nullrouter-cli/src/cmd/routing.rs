//! `nullrouter routing` (slice 006 contracts/operator-cli.md): the routing view, and the commands
//! that set per-account routing overrides and the amortization window.
//!
//! The view reads the running server over the operator socket (exit 4 with no server). The
//! `set`, `unset` and `window` subcommands edit `accounts.toml` and `config.toml` directly,
//! check the result with the registry gate's rules, and ask a running server to reload.
//! `set-plugin` and `unset-plugin` edit a plugin's `[provider.<id>.meter."<window>"]` tables in
//! `config.toml`: weights and multipliers only, since capacity is per account.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args as ClapArgs, Subcommand};
use nullrouter_cli::routing_text;
use nullrouter_engine::accounts::{self, Accounts, PriceOverride, RoutingOverrides, WindowOverride};
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::{OperatorConfig, PartialTokenWeights, Percent, parse_duration};
use nullrouter_server::views;
use serde_json::json;

use super::connection::edit;

/// `routing [target]` shows the routing view; the subcommands change the settings.
#[derive(ClapArgs)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Only this unified model or `provider/model` target.
    target: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Set an account's routing overrides: `cache_lifetime=1h`, `reserve=10%`, `price.input=3`,
    /// `window.<name>.capacity=12000000` (also `.length`, `.reserve`), `window.<name>.weight.<class>=V`
    /// or `window.<name>.multiplier.<glob>=V`.
    Set {
        provider: String,
        name: String,
        #[arg(required = true, value_name = "KEY=VALUE")]
        settings: Vec<String>,
    },
    /// Remove overrides: `cache_lifetime`, `reserve`, `price`, `window.<name>` or one of its fields,
    /// `window.<name>.weight.<class>` or `window.<name>.multiplier.<glob>`.
    Unset {
        provider: String,
        name: String,
        #[arg(required = true, value_name = "KEY")]
        keys: Vec<String>,
    },
    /// Set a plugin's weights and multipliers for every account of it: `window.<name>.weight.<class>=V`
    /// or `window.<name>.multiplier.<glob>=V`. Capacity is per account and is refused here.
    SetPlugin {
        provider: String,
        #[arg(required = true, value_name = "KEY=VALUE")]
        settings: Vec<String>,
    },
    /// Remove a plugin's weights or multipliers: `window.<name>.weight.<class>` or
    /// `window.<name>.multiplier.<glob>`.
    UnsetPlugin {
        provider: String,
        #[arg(required = true, value_name = "KEY")]
        keys: Vec<String>,
    },
    /// Set the amortization length: `window 2h` for the default, `window sonnet 1h` for a target,
    /// `window default` or `window sonnet default` to go back to the default.
    Window {
        #[arg(required = true, num_args = 1..=2, value_name = "[TARGET] DURATION|default")]
        args: Vec<String>,
    },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

const KEYS: &str = "cache_lifetime, reserve, price.input, price.output, price.cache_read, price.cache_write, window.<name>.capacity, window.<name>.length, window.<name>.reserve, window.<name>.weight.<class>, window.<name>.multiplier.<glob>";

pub(crate) fn run(home: Option<PathBuf>, args: Args, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    match args.command {
        Some(Command::Set { provider, name, settings }) => set(&home, &provider, &name, &settings),
        Some(Command::Unset { provider, name, keys }) => unset(&home, &provider, &name, &keys),
        Some(Command::SetPlugin { provider, settings }) => set_plugin(&home, &provider, &settings),
        Some(Command::UnsetPlugin { provider, keys }) => unset_plugin(&home, &provider, &keys),
        Some(Command::Window { args }) => window(&home, &args),
        None => view(&home, args.target.as_deref(), as_json),
    }
}

fn view(home: &OperatorHome, target: Option<&str>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let view = super::read(home, views::routing::NEEDS, &json!({"target": target}), views::routing::build)?;
    if as_json {
        println!("{:#}", view.json);
    } else {
        print!("{}", routing_text::render(&view.json, nullrouter_engine::clock::now()));
    }
    Ok(ExitCode::SUCCESS)
}

/// Loads `accounts.toml`, lets `change` edit the named account's overrides, checks them against
/// the gate's rules and the provider's declared windows, then saves and tells the server.
fn edit_account(
    home: &OperatorHome,
    provider: &str,
    name: &str,
    change: impl FnOnce(&mut RoutingOverrides) -> Result<(), String>,
) -> Result<ExitCode, ExitCode> {
    let mut list = Accounts::load(&home.path().join(accounts::FILE)).map_err(fail)?;
    let mut routing = list
        .get(provider, name)
        .ok_or_else(|| fail(accounts::AccountError::NotFound { provider: provider.to_owned(), name: name.to_owned() }))?
        .routing
        .clone();
    change(&mut routing).map_err(fail)?;
    if let Some(problem) = routing.problem() {
        return Err(fail(problem));
    }
    let reg = crate::open(Some(home.path().to_owned()))?.snapshot();
    if let Ok(p) = reg.provider(provider)
        && let Some(w) = routing.unknown_windows(p.routing().windows).first()
    {
        return Err(fail(format!("window.{w}: {provider} declares no window with that name")));
    }
    if let Ok(p) = reg.provider(provider)
        && let Some(problem) = routing.meter_problem(provider, p.routing().windows)
    {
        return Err(fail(problem));
    }
    list.set_routing(provider, name, routing).map_err(fail)?;
    list.save().map_err(fail)?;
    let status = super::apply(home).map_err(fail)?;
    println!("{provider}/{name}: {status}");
    Ok(ExitCode::SUCCESS)
}

fn number(key: &str, v: &str) -> Result<f64, String> {
    v.parse::<f64>().ok().filter(|n| n.is_finite()).ok_or_else(|| format!("{key}: {v:?} is not a number"))
}

fn duration(key: &str, v: &str) -> Result<std::time::Duration, String> {
    parse_duration(v).map_err(|e| format!("{key}: {e}"))
}

fn percent(key: &str, v: &str) -> Result<Percent, String> {
    Percent::parse(v).map_err(|e| format!("{key}: {e}"))
}

/// `window.<name>.<field>` as `(name, field)`; the name may itself hold dots.
fn window_key(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix("window.")?;
    let (name, field) = rest.rsplit_once('.')?;
    matches!(field, "capacity" | "length" | "reserve").then_some((name, field))
}

fn set(home: &OperatorHome, provider: &str, name: &str, settings: &[String]) -> Result<ExitCode, ExitCode> {
    edit_account(home, provider, name, |r| {
        let mut price: Option<PriceOverride> = r.price;
        let mut price_extra: Vec<(&str, f64)> = Vec::new();
        for setting in settings {
            let (key, value) = setting.split_once('=').ok_or_else(|| format!("{setting}: expected key=value"))?;
            match key {
                "cache_lifetime" => r.cache_lifetime = Some(duration(key, value)?),
                "reserve" => r.reserve = Some(percent(key, value)?),
                "price.input" => {
                    let input = number(key, value)?;
                    price = Some(PriceOverride {
                        input,
                        ..price.unwrap_or(PriceOverride { input, output: None, cache_read: None, cache_write: None })
                    });
                }
                "price.output" | "price.cache_read" | "price.cache_write" => {
                    price_extra.push((key, number(key, value)?))
                }
                _ => match window_meter_key(key) {
                    Some((w, meter)) => {
                        let o = r.window.entry(w.to_owned()).or_default();
                        set_meter(o, key, meter, Some(number(key, value)?))?;
                    }
                    None => {
                        let (w, field) =
                            window_key(key).ok_or_else(|| format!("{key}: unknown setting; allowed: {KEYS}"))?;
                        let o = r.window.entry(w.to_owned()).or_default();
                        match field {
                            "capacity" => o.capacity = Some(number(key, value)?),
                            "length" => o.length = Some(duration(key, value)?),
                            _ => o.reserve = Some(percent(key, value)?),
                        }
                    }
                },
            }
        }
        if !price_extra.is_empty() {
            let p = price
                .as_mut()
                .ok_or_else(|| format!("{}: set price.input too; a price starts with its input", price_extra[0].0))?;
            for (key, v) in price_extra {
                match key {
                    "price.output" => p.output = Some(v),
                    "price.cache_read" => p.cache_read = Some(v),
                    _ => p.cache_write = Some(v),
                }
            }
        }
        r.price = price;
        Ok(())
    })
}

fn unset(home: &OperatorHome, provider: &str, name: &str, keys: &[String]) -> Result<ExitCode, ExitCode> {
    edit_account(home, provider, name, |r| {
        for key in keys {
            match key.as_str() {
                "cache_lifetime" => r.cache_lifetime = None,
                "reserve" => r.reserve = None,
                "price" | "price.input" => r.price = None,
                "price.output" | "price.cache_read" | "price.cache_write" => {
                    if let Some(p) = r.price.as_mut() {
                        match key.as_str() {
                            "price.output" => p.output = None,
                            "price.cache_read" => p.cache_read = None,
                            _ => p.cache_write = None,
                        }
                    }
                }
                other => match (window_meter_key(other), window_key(other), other.strip_prefix("window.")) {
                    (Some((w, meter)), _, _) => {
                        if let Some(o) = r.window.get_mut(w) {
                            set_meter(o, other, meter, None)?;
                            if *o == WindowOverride::default() {
                                r.window.remove(w);
                            }
                        }
                    }
                    (None, Some((w, field)), _) => {
                        if let Some(o) = r.window.get_mut(w) {
                            match field {
                                "capacity" => o.capacity = None,
                                "length" => o.length = None,
                                _ => o.reserve = None,
                            }
                            if *o == WindowOverride::default() {
                                r.window.remove(w);
                            }
                        }
                    }
                    (None, None, Some(w)) if !w.is_empty() => {
                        r.window.remove(w);
                    }
                    _ => return Err(format!("{other}: unknown setting; allowed: {KEYS}")),
                },
            }
        }
        Ok(())
    })
}

/// The last part of a `window.<name>.weight.<class>` or `window.<name>.multiplier.<glob>` key.
#[derive(Clone, Copy)]
enum Meter<'a> {
    Weight(&'a str),
    Multiplier(&'a str),
}

/// `window.<name>.weight.<class>` and `window.<name>.multiplier.<glob>` as `(name, meter)`. A glob
/// may hold dots, so a multiplier key is split at its first `.multiplier.`.
fn window_meter_key(key: &str) -> Option<(&str, Meter<'_>)> {
    let rest = key.strip_prefix("window.")?;
    if let Some((name, glob)) = rest.split_once(".multiplier.") {
        return (!name.is_empty()).then_some((name, Meter::Multiplier(glob)));
    }
    let (name, class) = rest.rsplit_once(".weight.")?;
    (!name.is_empty()).then_some((name, Meter::Weight(class)))
}

/// Sets (`Some`) or removes (`None`) the weight or multiplier that `meter` names in one window.
/// A window with no weights left has none, and an empty multiplier map is the same as none.
fn set_meter(o: &mut WindowOverride, key: &str, meter: Meter<'_>, value: Option<f64>) -> Result<(), String> {
    match meter {
        Meter::Weight(class) => {
            let mut t = o.token_weights.unwrap_or_default();
            match class {
                "input" => t.input = value,
                "output" => t.output = value,
                "cache_read" => t.cache_read = value,
                "cache_write" => t.cache_write = value,
                _ => {
                    return Err(format!("{key}: unknown weight class; allowed: input, output, cache_read, cache_write"));
                }
            }
            o.token_weights = (t != PartialTokenWeights::default()).then_some(t);
        }
        Meter::Multiplier(glob) => match value {
            Some(f) => {
                o.model_multiplier.insert(glob.to_owned(), f);
            }
            None => {
                o.model_multiplier.shift_remove(glob);
            }
        },
    }
    Ok(())
}

const PLUGIN_KEYS: &str = "window.<name>.weight.<class>, window.<name>.multiplier.<glob>";

/// One plugin-level setting (`Some` value to set, `None` to remove) on the per-window overrides.
/// Capacity is per account, so a plugin never takes it.
fn plugin_setting(meters: &mut BTreeMap<String, WindowOverride>, key: &str, value: Option<&str>) -> Result<(), String> {
    if matches!(window_key(key), Some((_, "capacity"))) {
        return Err("capacity is per account; use routing set <provider> <account>".into());
    }
    let (w, meter) = window_meter_key(key).ok_or_else(|| format!("{key}: unknown setting; allowed: {PLUGIN_KEYS}"))?;
    let value = value.map(|v| number(key, v)).transpose()?;
    let o = meters.entry(w.to_owned()).or_default();
    set_meter(o, key, meter, value)?;
    if *o == WindowOverride::default() {
        meters.remove(w);
    }
    Ok(())
}

/// `{ input = 1.0, output = 15.0 }`: the classes that are set, in the gate's order.
fn inline_weights(t: PartialTokenWeights) -> String {
    let parts: Vec<String> = [
        ("input", t.input),
        ("output", t.output),
        ("cache_read", t.cache_read),
        ("cache_write", t.cache_write),
    ]
    .into_iter()
    .filter_map(|(class, v)| v.map(|v| format!("{class} = {v:?}")))
    .collect();
    format!("{{ {} }}", parts.join(", "))
}

/// `{ "claude-opus-*" = 1.5 }`: the globs quoted, in the map's order.
fn inline_multipliers<'a>(m: impl Iterator<Item = (&'a String, &'a f64)>) -> String {
    let parts: Vec<String> = m.map(|(glob, f)| format!("\"{}\" = {f:?}", quoted(glob))).collect();
    format!("{{ {} }}", parts.join(", "))
}

/// A TOML basic-string body: backslashes and quotes escaped.
fn quoted(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Loads `config.toml`, lets `change` edit one plugin's per-window weights and multipliers, checks
/// the windows that changed with the gate's rules and the plugin's declared windows, rewrites
/// their `[provider.<id>.meter."<window>"]` tables, then saves and tells the server.
fn edit_plugin(
    home: &OperatorHome,
    provider: &str,
    change: impl FnOnce(&mut BTreeMap<String, WindowOverride>) -> Result<(), String>,
) -> Result<ExitCode, ExitCode> {
    let path = home.config_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(fail(format!("{}: {e}", path.display()))),
    };
    let config: OperatorConfig = toml::from_str(&text).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let reg = crate::open(Some(home.path().to_owned()))?.snapshot();
    let p = reg.provider(provider).map_err(|_| {
        let known: Vec<&str> = reg.providers().map(|p| p.id.as_str()).collect();
        fail(format!("no provider {provider:?}; known providers: {}", known.join(", ")))
    })?;
    let id = p.id.clone();
    let declared = p.routing().windows;

    let before: BTreeMap<String, WindowOverride> = config
        .provider
        .get(&id)
        .map(|s| {
            s.meter
                .iter()
                .map(|(w, m)| {
                    let o = WindowOverride {
                        token_weights: m.token_weights,
                        model_multiplier: m.model_multiplier.clone(),
                        ..Default::default()
                    };
                    (w.clone(), o)
                })
                .collect()
        })
        .unwrap_or_default();
    let mut after = before.clone();
    change(&mut after).map_err(fail)?;
    let changed: BTreeSet<String> =
        before.keys().chain(after.keys()).filter(|n| before.get(*n) != after.get(*n)).cloned().collect();
    if changed.is_empty() {
        println!("{id}: no change");
        return Ok(ExitCode::SUCCESS);
    }
    for name in &changed {
        let Some(o) = after.get(name) else { continue };
        let one = RoutingOverrides { window: BTreeMap::from([(name.clone(), o.clone())]), ..Default::default() };
        if let Some(problem) = one.problem() {
            return Err(fail(problem));
        }
        if !one.unknown_windows(declared).is_empty() {
            return Err(fail(format!("window.{name}: {id} declares no window with that name")));
        }
        if let Some(problem) = one.meter_problem(&id, declared) {
            return Err(fail(problem));
        }
    }

    let mut edited = text;
    for name in &changed {
        let header = format!("provider.{id}.meter.\"{}\"", quoted(name));
        let (weights, multipliers) = match after.get(name) {
            Some(o) => (
                o.token_weights.map(inline_weights),
                (!o.model_multiplier.is_empty()).then(|| inline_multipliers(o.model_multiplier.iter())),
            ),
            None => (None, None),
        };
        edited = edit(&edited, &header, "token_weights", weights.as_deref()).0;
        edited = edit(&edited, &header, "model_multiplier", multipliers.as_deref()).0;
    }
    // The file must still load: a config the server would refuse is never written.
    toml::from_str::<OperatorConfig>(&edited)
        .map_err(|e| fail(format!("{}: the edit would not load: {e}", path.display())))?;
    std::fs::create_dir_all(home.path()).map_err(fail)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &edited)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let status = super::apply(home).map_err(fail)?;
    println!("{id} plugin: {status}");
    Ok(ExitCode::SUCCESS)
}

fn set_plugin(home: &OperatorHome, provider: &str, settings: &[String]) -> Result<ExitCode, ExitCode> {
    edit_plugin(home, provider, |meters| {
        for setting in settings {
            let (key, value) = setting.split_once('=').ok_or_else(|| format!("{setting}: expected key=value"))?;
            plugin_setting(meters, key, Some(value))?;
        }
        Ok(())
    })
}

fn unset_plugin(home: &OperatorHome, provider: &str, keys: &[String]) -> Result<ExitCode, ExitCode> {
    edit_plugin(home, provider, |meters| {
        for key in keys {
            plugin_setting(meters, key, None)?;
        }
        Ok(())
    })
}

fn window(home: &OperatorHome, args: &[String]) -> Result<ExitCode, ExitCode> {
    let (target, length) = match args {
        [length] => (None, length.as_str()),
        [target, length] => (Some(target.as_str()), length.as_str()),
        _ => return Err(fail("usage: routing window [<target>] <duration|default>")),
    };
    let value = if length == "default" {
        None
    } else {
        let d = parse_duration(length).map_err(|e| fail(format!("amortization window: {e}")))?;
        if d.is_zero() {
            return Err(fail("an amortization window must be more than 0"));
        }
        Some(length)
    };
    let path = home.config_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(fail(format!("{}: {e}", path.display()))),
    };
    let text = match target {
        None => edit_key(&text, "[routing]", "amortization", value.map(|v| format!("\"{v}\"")).as_deref()),
        Some(t) => edit_key(&text, "[routing.amortization_for]", t, value.map(|v| format!("\"{v}\"")).as_deref()),
    };
    // The file must still load: a config the server would refuse is never written.
    toml::from_str::<OperatorConfig>(&text)
        .map_err(|e| fail(format!("{}: the edit would not load: {e}", path.display())))?;
    std::fs::create_dir_all(home.path()).map_err(fail)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &text)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let status = super::apply(home).map_err(fail)?;
    let what = target.map_or_else(|| "amortization".to_owned(), |t| format!("amortization for {t}"));
    println!("{what} = {}: {status}", value.unwrap_or("default"));
    Ok(ExitCode::SUCCESS)
}

/// `text` with `key` in the table `header` set to `value` (a TOML value), or removed when `value`
/// is `None`. Every other line is kept as written. A target key is written quoted; a key of
/// `[routing]` or a `[tests…]` table bare.
pub(crate) fn edit_key(text: &str, header: &str, key: &str, value: Option<&str>) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let is_header = |l: &str| l.trim_start().starts_with('[');
    let bare = header == "[routing]" || header.starts_with("[tests");
    let written = if bare { key.to_owned() } else { format!("\"{key}\"") };
    let Some(start) = lines.iter().position(|l| l.trim() == header) else {
        let Some(v) = value else { return text.to_owned() };
        let mut out = text.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{header}\n{written} = {v}\n"));
        return out;
    };
    let end = lines[start + 1..].iter().position(|l| is_header(l)).map_or(lines.len(), |i| start + 1 + i);
    let is_key = |l: &str| l.split_once('=').is_some_and(|(k, _)| k.trim().trim_matches('"') == key);
    let found = (start + 1..end).find(|&i| is_key(&lines[i]));
    match (found, value) {
        (Some(i), Some(v)) => lines[i] = format!("{written} = {v}"),
        (Some(i), None) => {
            lines.remove(i);
        }
        (None, Some(v)) => lines.insert(start + 1, format!("{written} = {v}")),
        (None, None) => {}
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_added_replaced_removed_or_given_a_table() {
        assert_eq!(edit_key("", "[routing]", "amortization", Some("\"2h\"")), "[routing]\namortization = \"2h\"\n");
        let with = "schema = 1\n[routing]\n# note\namortization = \"5h\"\n[server]\nlisten = \"x\"\n";
        assert_eq!(
            edit_key(with, "[routing]", "amortization", Some("\"2h\"")),
            "schema = 1\n[routing]\n# note\namortization = \"2h\"\n[server]\nlisten = \"x\"\n"
        );
        assert_eq!(
            edit_key(with, "[routing]", "amortization", None),
            "schema = 1\n[routing]\n# note\n[server]\nlisten = \"x\"\n"
        );
        assert_eq!(edit_key("a = 1\n", "[routing]", "amortization", None), "a = 1\n");
        let targets = "[routing.amortization_for]\n\"sonnet\" = \"1h\"\n";
        assert_eq!(
            edit_key(targets, "[routing.amortization_for]", "sonnet", Some("\"2h\"")),
            "[routing.amortization_for]\n\"sonnet\" = \"2h\"\n"
        );
        assert_eq!(edit_key(targets, "[routing.amortization_for]", "sonnet", None), "[routing.amortization_for]\n");
        assert_eq!(
            edit_key("a = 1", "[routing.amortization_for]", "x/y", Some("\"1h\"")),
            "a = 1\n\n[routing.amortization_for]\n\"x/y\" = \"1h\"\n"
        );
    }

    #[test]
    fn window_keys_split_at_the_field() {
        assert_eq!(window_key("window.5-hour.capacity"), Some(("5-hour", "capacity")));
        assert_eq!(window_key("window.weekly *.reserve"), Some(("weekly *", "reserve")));
        assert_eq!(window_key("window.5-hour"), None);
        assert_eq!(window_key("window.x.colour"), None);
    }
}
