//! The connection operations the CLI and the dashboard share (`connection set`, `unset`): a
//! timeout, switch or retry rule of a provider, or a timeout of one of its models, written to
//! `config.toml`. Nothing here prints or exits; the notes the CLI prints on stderr come back in
//! `log`.
//!
//! The retry keys `retries[.STATUS]` and `retry-wait[.STATUS]` edit `[provider.P.retry]`, where
//! a status (or `all`) holds one rule `{ retries, delay_ms }`: unsetting either key removes the
//! rule, and `retry-wait` needs the rule's `retries` to be set first.

use nullrouter_registry::schema::{OperatorConfig, parse_duration};

use super::accounts::fail;
use super::config_text::{edit, quoted, write_checked};
use super::{Ctx, Done, OpError, Status, apply};

fn fail_in(field: &'static str, e: impl std::fmt::Display) -> OpError {
    OpError { text: e.to_string(), exit: 1, field: Some(field) }
}

fn done(text: String) -> Done {
    Done { status: Status::Applied, text, once: None }
}

/// A reload the running server refused: the file is saved, the reason is the text.
fn refused(reason: String) -> Done {
    Done { status: Status::SavedNotApplied { reason: reason.clone() }, text: reason, once: None }
}

/// Sets a timeout, switch or retry count for a provider, or for one of its models. `log` gets
/// the notes the reload raised, in order.
pub fn set_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    model: Option<&str>,
    key: &str,
    value: &str,
) -> Result<Done, OpError> {
    change(ctx, log, provider, model, key, Some(value))
}

pub fn set(ctx: &Ctx, provider: &str, model: Option<&str>, key: &str, value: &str) -> Result<Done, OpError> {
    set_with(ctx, &mut Vec::new(), provider, model, key, value)
}

/// Removes a setting for a provider, or for one of its models. `log` gets the notes the reload
/// raised, in order.
pub fn unset_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    model: Option<&str>,
    key: &str,
) -> Result<Done, OpError> {
    change(ctx, log, provider, model, key, None)
}

pub fn unset(ctx: &Ctx, provider: &str, model: Option<&str>, key: &str) -> Result<Done, OpError> {
    unset_with(ctx, &mut Vec::new(), provider, model, key)
}

/// The config key for a CLI key.
fn config_key(key: &str) -> Result<&'static str, String> {
    Ok(match key {
        "connect-timeout" => "connect_timeout_ms",
        "header-timeout" => "header_timeout_ms",
        "first-token-timeout" => "first_token_timeout_ms",
        "stall-timeout" => "stall_timeout_ms",
        "reuse" => "reuse",
        "http2" => "http2",
        _ => {
            return Err(format!(
                "unknown key {key:?}; allowed: connect-timeout, header-timeout, first-token-timeout, stall-timeout, reuse, http2"
            ));
        }
    })
}

/// Which half of a retry rule a key sets.
#[derive(Clone, Copy, PartialEq)]
enum RetryField {
    Retries,
    Wait,
}

/// `retries`, `retries.503`, `retry-wait` and `retry-wait.503` as the field and the table key
/// (`all` or the status). `None` for any other key.
fn retry_key(key: &str) -> Option<Result<(RetryField, String), String>> {
    let (name, status) = key.split_once('.').map_or((key, None), |(n, s)| (n, Some(s)));
    let field = match name {
        "retries" => RetryField::Retries,
        "retry-wait" => RetryField::Wait,
        _ => return None,
    };
    Some(match status {
        None => Ok((field, "all".into())),
        Some(s) if s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit()) => Ok((field, s.into())),
        Some(s) => Err(format!("{key}: {s:?} is not a 3-digit HTTP status")),
    })
}

/// `text` with the retry rule for `status` changed in `[provider.P.retry]`.
fn edit_retry(
    text: &str,
    provider: &str,
    (field, status): (RetryField, &str),
    value: Option<&str>,
) -> Result<(String, bool), String> {
    let header = format!("provider.{provider}.retry");
    let table_key = if status == "all" { "all".to_owned() } else { format!("\"{status}\"") };
    let Some(v) = value else { return Ok(edit(text, &header, &table_key, None)) };
    let config: OperatorConfig = toml::from_str(text).map_err(|e| format!("config.toml: {e}"))?;
    let retry = config.provider.get(provider).map(|p| &p.retry);
    let current = retry.and_then(|r| if status == "all" { r.all } else { r.by_status.get(status).copied() });
    let (retries, delay_ms) = match field {
        RetryField::Retries => {
            let n: u32 = v.parse().map_err(|_| format!("retries takes a count 0-5, not {v:?}"))?;
            (n, current.map_or(0, |c| c.delay_ms))
        }
        RetryField::Wait => {
            let Some(c) = current else {
                return Err(format!("set retries{} first; a wait needs a retry count", dotted(status)));
            };
            let ms = parse_duration(v).map_err(|e| format!("retry-wait: {e}"))?.as_millis();
            (c.retries, u64::try_from(ms).map_err(|_| format!("retry-wait: {v} is too long"))?)
        }
    };
    Ok(edit(text, &header, &table_key, Some(&format!("{{ retries = {retries}, delay_ms = {delay_ms} }}"))))
}

fn dotted(status: &str) -> String {
    if status == "all" { String::new() } else { format!(".{status}") }
}

/// `reuse` and `http2` are switches kept for a provider, never a model.
fn is_switch(key: &str) -> bool {
    matches!(key, "reuse" | "http2")
}

/// `on` or `off`, as the TOML boolean it is written as.
fn parse_switch(key: &str, value: &str) -> Result<String, String> {
    match value {
        "on" => Ok("true".into()),
        "off" => Ok("false".into()),
        _ => Err(format!("{key} takes on or off, not {value:?}")),
    }
}

/// A timeout in ms from `500ms`, `30s`, `5m`; `off` is 0 and only the first token takes it.
fn parse_value(key: &str, value: &str) -> Result<u64, String> {
    if value == "off" {
        return if key == "first-token-timeout" {
            Ok(0)
        } else {
            Err(format!("{key} can't be off; give a duration such as 30s"))
        };
    }
    let ms = parse_duration(value).map_err(|e| format!("{key}: {e}"))?.as_millis();
    u64::try_from(ms).map_err(|_| format!("{key}: {value} is too long"))
}

/// Changes (`Some`) or removes (`None`) one setting in `config.toml`, then asks the server to
/// reload. Nothing is written for an unknown provider or a value the config would refuse.
fn change(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    model: Option<&str>,
    key: &str,
    value: Option<&str>,
) -> Result<Done, OpError> {
    let retry = retry_key(key).transpose().map_err(|e| fail_in("key", e))?;
    if retry.is_some() && model.is_some() {
        return Err(fail_in("model", format!("{key} is set for a provider, not a model")));
    }
    let ckey = if retry.is_some() { "" } else { config_key(key).map_err(|e| fail_in("key", e))? };
    if is_switch(key) && model.is_some() {
        return Err(fail_in("model", format!("{key} is set for a provider, not a model")));
    }
    let new = match value {
        _ if retry.is_some() => None,
        Some(v) if is_switch(key) => Some(parse_switch(key, v).map_err(|e| fail_in("value", e))?),
        Some(v) => Some(parse_value(key, v).map_err(|e| fail_in("value", e))?.to_string()),
        None => None,
    };
    let handle = crate::views::open_registry(&ctx.home).map_err(|e| fail(e.message))?;
    let snapshot = handle.snapshot();
    let known: Vec<&str> = snapshot.providers().map(|p| p.id.as_str()).collect();
    if !known.contains(&provider) {
        return Err(fail_in("provider", format!("no provider {provider:?}; known providers: {}", known.join(", "))));
    }
    let header = match model {
        Some(m) => format!("provider.{provider}.model.\"{}\".connection", quoted(m)),
        None => format!("provider.{provider}.connection"),
    };
    let _lock = super::config_text::lock(&ctx.home);
    let path = ctx.home.config_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(fail(format!("{}: {e}", path.display()))),
    };
    let what = format!("{provider}{} {key}", model.map_or(String::new(), |m| format!(" model {m}")));
    let (edited, changed) = match &retry {
        Some((field, status)) => edit_retry(&text, provider, (*field, status.as_str()), value).map_err(fail)?,
        None => edit(&text, &header, ckey, new.as_deref()),
    };
    if !changed {
        return Ok(done(format!("{what}: was not set")));
    }
    write_checked(&ctx.home, &edited).map_err(fail)?;
    let applied = match apply(ctx) {
        Ok(a) => a,
        Err(reason) => return Ok(refused(reason)),
    };
    for n in &applied.notes {
        log.push(format!("note: {n}"));
    }
    let status = applied.text;
    Ok(done(match value {
        Some(v) => format!("{what} = {v}: {status}"),
        None => format!("{what} unset: {status}"),
    }))
}

#[cfg(test)]
mod tests {
    use nullrouter_registry::OperatorHome;

    use super::*;

    #[test]
    fn values_are_durations_and_only_first_token_may_be_off() {
        assert_eq!(parse_value("header-timeout", "500ms"), Ok(500));
        assert_eq!(parse_value("header-timeout", "30s"), Ok(30_000));
        assert_eq!(parse_value("stall-timeout", "5m"), Ok(300_000));
        assert_eq!(parse_value("first-token-timeout", "off"), Ok(0));
        assert!(parse_value("header-timeout", "off").unwrap_err().contains("can't be off"));
        assert!(parse_value("header-timeout", "soon").unwrap_err().contains("header-timeout"));
        assert!(config_key("retries").unwrap_err().contains("allowed"));
    }

    fn ctx_with_config(config: &str) -> (tempfile::TempDir, Ctx) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        let ctx = Ctx { home: OperatorHome::new(dir.path()), engine: None };
        (dir, ctx)
    }

    #[test]
    fn an_unknown_provider_is_refused_with_the_known_ones_and_nothing_is_written() {
        let (dir, ctx) = ctx_with_config("schema = 1\n");
        let e = set(&ctx, "nope", None, "header-timeout", "10s").unwrap_err();
        assert!(e.text.contains("no provider \"nope\"") && e.text.contains("known providers:"), "{}", e.text);
        assert_eq!((e.exit, e.field), (1, Some("provider")));
        assert_eq!(std::fs::read_to_string(dir.path().join("config.toml")).unwrap(), "schema = 1\n");
    }
}
