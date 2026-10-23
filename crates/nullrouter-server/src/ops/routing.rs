//! The routing operations the CLI and the dashboard share: per-account overrides (`routing set`,
//! `unset`), a plugin's weights and multipliers (`set-plugin`, `unset-plugin`), the amortization
//! window (`window`), and the operator's break behaviour (`behaviour set-break`). Nothing here
//! prints or exits: what the CLI prints on stderr comes back in `log`, in order.

use std::collections::{BTreeMap, BTreeSet};

use nullrouter_engine::accounts::{self, Accounts, PriceOverride, RoutingOverrides, WindowOverride};
use nullrouter_registry::schema::OperatorConfig;
use nullrouter_registry::schema::{BreakBehaviour, PartialTokenWeights, Percent, parse_duration};

use super::accounts::{Report, fail, refused};
use super::config_text::{self, edit, quoted, write_checked};
use super::{Ctx, Done, OpError, Status, apply};

fn fail_in(field: &'static str, e: impl std::fmt::Display) -> OpError {
    OpError { text: e.to_string(), exit: 1, field: Some(field) }
}

fn open(ctx: &Ctx) -> Result<nullrouter_registry::RegistryHandle, OpError> {
    crate::views::open_registry(&ctx.home).map_err(|e| OpError { text: e.message, exit: e.code, field: None })
}

/// Reloads and builds the report whose text is `line(status)`.
fn finish(ctx: &Ctx, log: &mut Vec<String>, line: impl FnOnce(&str) -> String) -> Result<Report, OpError> {
    let applied = match apply(ctx) {
        Ok(a) => a,
        Err(reason) => return Ok(refused(reason)),
    };
    for n in &applied.notes {
        log.push(format!("note: {n}"));
    }
    let status = applied.text;
    let text = line(status);
    Ok(Report { done: Done { status: Status::Applied, text, once: None }, json: serde_json::Value::Null, status })
}

fn read_config(ctx: &Ctx) -> Result<String, OpError> {
    let path = ctx.home.config_file();
    match std::fs::read_to_string(&path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(fail(format!("{}: {e}", path.display()))),
    }
}

/// Sets the operator default for a stream that breaks after output: `restart` or `error_event`.
pub fn set_break_default_with(ctx: &Ctx, log: &mut Vec<String>, behaviour: &str) -> Result<Report, OpError> {
    let b = BreakBehaviour::parse(behaviour).ok_or_else(|| {
        fail_in("behaviour", format!("unknown break behaviour {behaviour:?}; allowed: restart, error_event"))
    })?;
    let _lock = super::config_text::lock(&ctx.home);
    let text = set_break(&read_config(ctx)?, b);
    write_checked(&ctx.home, &text).map_err(fail)?;
    finish(ctx, log, |status| format!("break_behaviour = {}: {status}", b.as_str()))
}

pub fn set_break_default(ctx: &Ctx, behaviour: &str) -> Result<Done, OpError> {
    set_break_default_with(ctx, &mut Vec::new(), behaviour).map(|r| r.done)
}

/// Loads `accounts.toml`, lets `change` edit the named account's overrides, checks them against
/// the gate's rules and the provider's declared windows, then saves and tells the server.
fn edit_account(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    name: &str,
    change: impl FnOnce(&mut RoutingOverrides) -> Result<(), String>,
) -> Result<Report, OpError> {
    let mut list = Accounts::load(&ctx.home.path().join(accounts::FILE)).map_err(fail)?;
    let mut routing = list
        .get(provider, name)
        .ok_or_else(|| {
            fail_in("name", accounts::AccountError::NotFound { provider: provider.to_owned(), name: name.to_owned() })
        })?
        .routing
        .clone();
    change(&mut routing).map_err(|e| fail_in("value", e))?;
    if let Some(problem) = routing.problem() {
        return Err(fail_in("value", problem));
    }
    let reg = open(ctx)?.snapshot();
    if let Ok(p) = reg.provider(provider)
        && let Some(w) = routing.unknown_windows(p.routing().windows).first()
    {
        return Err(fail_in("key", format!("window.{w}: {provider} declares no window with that name")));
    }
    if let Ok(p) = reg.provider(provider)
        && let Some(problem) = routing.meter_problem(provider, p.routing().windows)
    {
        return Err(fail_in("value", problem));
    }
    list.set_routing(provider, name, routing).map_err(fail)?;
    list.save().map_err(fail)?;
    finish(ctx, log, |status| format!("{provider}/{name}: {status}"))
}

const KEYS: &str = "cache_lifetime, reserve, price.input, price.output, price.cache_read, price.cache_write, window.<name>.capacity, window.<name>.length, window.<name>.reserve, window.<name>.weight.<class>, window.<name>.multiplier.<glob>";

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

pub fn set_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    name: &str,
    settings: &[String],
) -> Result<Report, OpError> {
    edit_account(ctx, log, provider, name, |r| {
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

pub fn set(ctx: &Ctx, provider: &str, name: &str, settings: &[String]) -> Result<Done, OpError> {
    set_with(ctx, &mut Vec::new(), provider, name, settings).map(|r| r.done)
}

pub fn unset_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    name: &str,
    keys: &[String],
) -> Result<Report, OpError> {
    edit_account(ctx, log, provider, name, |r| {
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

pub fn unset(ctx: &Ctx, provider: &str, name: &str, keys: &[String]) -> Result<Done, OpError> {
    unset_with(ctx, &mut Vec::new(), provider, name, keys).map(|r| r.done)
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
                    return Err(format!(
                        "{key}: unknown weight class; allowed: input, output, cache_read, cache_write"
                    ));
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
    let parts: Vec<String> =
        [("input", t.input), ("output", t.output), ("cache_read", t.cache_read), ("cache_write", t.cache_write)]
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

/// Loads `config.toml`, lets `change` edit one plugin's per-window weights and multipliers, checks
/// the windows that changed with the gate's rules and the plugin's declared windows, rewrites
/// their `[provider.<id>.meter."<window>"]` tables, then saves and tells the server.
fn edit_plugin(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    change: impl FnOnce(&mut BTreeMap<String, WindowOverride>) -> Result<(), String>,
) -> Result<Report, OpError> {
    let path = ctx.home.config_file();
    let _lock = super::config_text::lock(&ctx.home);
    let text = read_config(ctx)?;
    let config: OperatorConfig = toml::from_str(&text).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let reg = open(ctx)?.snapshot();
    let p = reg.provider(provider).map_err(|_| {
        let known: Vec<&str> = reg.providers().map(|p| p.id.as_str()).collect();
        fail_in("provider", format!("no provider {provider:?}; known providers: {}", known.join(", ")))
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
    change(&mut after).map_err(|e| fail_in("value", e))?;
    let changed: BTreeSet<String> =
        before.keys().chain(after.keys()).filter(|n| before.get(*n) != after.get(*n)).cloned().collect();
    if changed.is_empty() {
        let status = "";
        let text = format!("{id}: no change");
        return Ok(Report {
            done: Done { status: Status::Applied, text, once: None },
            json: serde_json::Value::Null,
            status,
        });
    }
    for name in &changed {
        let Some(o) = after.get(name) else { continue };
        let one = RoutingOverrides { window: BTreeMap::from([(name.clone(), o.clone())]), ..Default::default() };
        if let Some(problem) = one.problem() {
            return Err(fail_in("value", problem));
        }
        if !one.unknown_windows(declared).is_empty() {
            return Err(fail_in("key", format!("window.{name}: {id} declares no window with that name")));
        }
        if let Some(problem) = one.meter_problem(&id, declared) {
            return Err(fail_in("value", problem));
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
    write_checked(&ctx.home, &edited).map_err(fail)?;
    finish(ctx, log, |status| format!("{id} plugin: {status}"))
}

pub fn set_plugin_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    settings: &[String],
) -> Result<Report, OpError> {
    edit_plugin(ctx, log, provider, |meters| {
        for setting in settings {
            let (key, value) = setting.split_once('=').ok_or_else(|| format!("{setting}: expected key=value"))?;
            plugin_setting(meters, key, Some(value))?;
        }
        Ok(())
    })
}

pub fn set_plugin(ctx: &Ctx, provider: &str, settings: &[String]) -> Result<Done, OpError> {
    set_plugin_with(ctx, &mut Vec::new(), provider, settings).map(|r| r.done)
}

pub fn unset_plugin_with(ctx: &Ctx, log: &mut Vec<String>, provider: &str, keys: &[String]) -> Result<Report, OpError> {
    edit_plugin(ctx, log, provider, |meters| {
        for key in keys {
            plugin_setting(meters, key, None)?;
        }
        Ok(())
    })
}

pub fn unset_plugin(ctx: &Ctx, provider: &str, keys: &[String]) -> Result<Done, OpError> {
    unset_plugin_with(ctx, &mut Vec::new(), provider, keys).map(|r| r.done)
}

/// Sets the amortization length for the default (`target` `None`) or one target; `length` is a
/// duration or `default` to go back to the default.
pub fn window_with(ctx: &Ctx, log: &mut Vec<String>, target: Option<&str>, length: &str) -> Result<Report, OpError> {
    let value = if length == "default" {
        None
    } else {
        let d = parse_duration(length).map_err(|e| fail_in("duration", format!("amortization window: {e}")))?;
        if d.is_zero() {
            return Err(fail_in("duration", "an amortization window must be more than 0"));
        }
        Some(length)
    };
    let _lock = super::config_text::lock(&ctx.home);
    if let Some(t) = target.filter(|t| t.chars().any(|c| c.is_control())) {
        return Err(fail_in("target", format!("{t:?}: a target has no control characters")));
    }
    let text = read_config(ctx)?;
    let quoted_value = value.map(|v| format!("\"{v}\""));
    let text = match target {
        None => edit_key(&text, "[routing]", "amortization", quoted_value.as_deref()),
        Some(t) => edit_key(&text, "[routing.amortization_for]", t, quoted_value.as_deref()),
    };
    // The loader's check names a target that is no unified model and no provider/model, which the
    // next load would refuse; nothing is written then.
    config_text::check(&ctx.home, &text).map_err(|e| fail_in("target", e))?;
    config_text::write_file(&ctx.home, &text).map_err(fail)?;
    finish(ctx, log, |status| {
        let what = target.map_or_else(|| "amortization".to_owned(), |t| format!("amortization for {t}"));
        format!("{what} = {}: {status}", value.unwrap_or("default"))
    })
}

pub fn window(ctx: &Ctx, target: Option<&str>, length: &str) -> Result<Done, OpError> {
    window_with(ctx, &mut Vec::new(), target, length).map(|r| r.done)
}

/// `text` with `key` in the table `header` set to `value` (a TOML value), or removed when `value`
/// is `None`. Every other line is kept as written. A target key is written quoted; a key of
/// `[routing]` or a `[tests…]` table bare.
pub fn edit_key(text: &str, header: &str, key: &str, value: Option<&str>) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let is_header = |l: &str| l.trim_start().starts_with('[');
    let bare = header == "[routing]" || header.starts_with("[tests");
    let written = if bare { key.to_owned() } else { format!("\"{}\"", config_text::quoted(key)) };
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

/// `text` with `[pipeline] break_behaviour` set to `b`; every other line kept as written.
fn set_break(text: &str, b: BreakBehaviour) -> String {
    let line = format!("break_behaviour = \"{}\"", b.as_str());
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let header = |l: &str| l.trim_start().starts_with('[');
    let Some(start) = lines.iter().position(|l| l.trim() == "[pipeline]") else {
        let mut out = text.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("[pipeline]\n");
        out.push_str(&line);
        out.push('\n');
        return out;
    };
    let end = lines[start + 1..].iter().position(|l| header(l)).map_or(lines.len(), |i| start + 1 + i);
    let key = |l: &str| l.split_once('=').is_some_and(|(k, _)| k.trim() == "break_behaviour");
    match (start + 1..end).find(|&i| key(&lines[i])) {
        Some(i) => lines[i] = line,
        None => lines.insert(start + 1, line),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_for_a_target_that_names_nothing_is_refused_in_the_target_field_and_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Ctx { home: nullrouter_registry::OperatorHome::new(dir.path()), engine: None };
        let e = window(&ctx, Some("sonnet"), "2h").unwrap_err();
        assert_eq!((e.exit, e.field), (1, Some("target")));
        assert!(e.text.contains("sonnet"), "{}", e.text);
        assert!(!ctx.home.config_file().exists(), "nothing written");
    }

    #[test]
    fn a_target_with_a_quote_and_a_newline_is_refused_and_config_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Ctx { home: nullrouter_registry::OperatorHome::new(dir.path()), engine: None };
        let original = "schema = 1\n";
        std::fs::write(ctx.home.config_file(), original).unwrap();
        let target = "sonnet\" = \"1h\"\n[server]\nlisten = \"x\"\n#";
        let e = window(&ctx, Some(target), "2h").unwrap_err();
        assert_eq!((e.exit, e.field), (1, Some("target")));
        assert_eq!(std::fs::read_to_string(ctx.home.config_file()).unwrap(), original);
    }

    #[test]
    fn a_quote_in_a_key_is_escaped() {
        let out = edit_key("", "[routing.amortization_for]", "a\"b", Some("\"1h\""));
        assert!(out.contains("\"a\\\"b\" = \"1h\""), "{out}");
    }

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

    #[test]
    fn the_setting_is_added_replaced_or_given_a_table() {
        assert_eq!(set_break("", BreakBehaviour::ErrorEvent), "[pipeline]\nbreak_behaviour = \"error_event\"\n");
        let with = "schema = 1\n[pipeline]\n# note\nbreak_behaviour = \"restart\"\n[server]\nlisten = \"x\"\n";
        assert_eq!(
            set_break(with, BreakBehaviour::ErrorEvent),
            "schema = 1\n[pipeline]\n# note\nbreak_behaviour = \"error_event\"\n[server]\nlisten = \"x\"\n"
        );
        assert_eq!(
            set_break("[pipeline]\n[server]\n", BreakBehaviour::Restart),
            "[pipeline]\nbreak_behaviour = \"restart\"\n[server]\n"
        );
        assert_eq!(
            set_break("schema = 1", BreakBehaviour::Restart),
            "schema = 1\n\n[pipeline]\nbreak_behaviour = \"restart\"\n"
        );
    }
}
