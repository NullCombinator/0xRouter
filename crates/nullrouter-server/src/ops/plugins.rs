//! The plugin operations the CLI and the dashboard share: `plugins install`, `uninstall`,
//! `private-endpoints`, and the custom compatible provider (`providers add-compatible`).
//! Nothing here prints or exits.

use std::path::Path;

use nullrouter_registry::community::{self, InstallError};
use nullrouter_registry::compatible::{self, Spec, Style};
use nullrouter_registry::{check_user_plugin, load_config, validate};
use serde_json::json;

use super::accounts::{Report, fail, refused};
use super::config_text;
use super::{Ctx, Done, OpError, Status, apply};

/// Exit code for a plugin this core can't support, or an address the setting forbids.
const REFUSED: u8 = 3;

fn install_error(e: &InstallError) -> OpError {
    let exit = if matches!(e, InstallError::Unsupported(_)) { REFUSED } else { 1 };
    OpError { text: e.to_string(), exit, field: None }
}

fn in_field(field: &'static str, exit: u8, text: impl Into<String>) -> OpError {
    OpError { text: text.into(), exit, field: Some(field) }
}

/// Reloads and builds the report: `line` is the text before the status word.
fn finish(ctx: &Ctx, log: &mut Vec<String>, line: &str, mut json: serde_json::Value) -> Report {
    match apply(ctx) {
        Ok(a) => {
            for n in &a.notes {
                log.push(format!("note: {n}"));
            }
            json["status"] = a.text.into();
            Report {
                done: Done { status: Status::Applied, text: format!("{line}, {}", a.text), once: None },
                json,
                status: a.text,
            }
        }
        Err(reason) => refused(format!("{line}, {reason}")),
    }
}

fn path_report(ctx: &Ctx, log: &mut Vec<String>, what: &str, id: &str, path: &Path) -> Report {
    let shown = path.display().to_string();
    finish(ctx, log, &format!("{what} {id} ({shown})"), json!({"id": id, what: shown}))
}

/// Installs a community plugin: the gate and the fit check, then the copy.
pub fn install_with(ctx: &Ctx, log: &mut Vec<String>, id: &str) -> Result<Report, OpError> {
    let path = community::install(id, &ctx.home).map_err(|e| install_error(&e))?;
    Ok(path_report(ctx, log, "installed", id, &path))
}

pub fn install(ctx: &Ctx, id: &str) -> Result<Done, OpError> {
    install_with(ctx, &mut Vec::new(), id).map(|r| r.done)
}

/// The sentence the CLI's `[y/N]` prompt and the dashboard's confirm window show.
#[must_use]
pub fn uninstall_question(id: &str) -> String {
    super::describe_loss("plugins.uninstall", id).unwrap_or_default()
}

/// Uninstalls a community plugin and drops its fit state.
pub fn uninstall_with(ctx: &Ctx, log: &mut Vec<String>, id: &str) -> Result<Report, OpError> {
    let path = community::uninstall(id, &ctx.home).map_err(|e| install_error(&e))?;
    // The plugin's fits go with it; reinstalling starts from nothing (spec 012).
    if let Err(e) = nullrouter_engine::quota::fit::store::remove(ctx.home.path(), id) {
        log.push(format!("fit state of {id} not removed: {e}"));
    }
    Ok(path_report(ctx, log, "uninstalled", id, &path))
}

pub fn uninstall(ctx: &Ctx, id: &str) -> Result<Done, OpError> {
    uninstall_with(ctx, &mut Vec::new(), id).map(|r| r.done)
}

/// The question the CLI asks before turning private endpoints on.
#[must_use]
pub fn private_endpoints_question() -> String {
    super::describe_loss("plugins.private_endpoints", "on").unwrap_or_default()
}

/// Sets `allow_private_endpoints` in `config.toml` and reloads. Off removes the key.
pub fn private_endpoints_with(ctx: &Ctx, log: &mut Vec<String>, on: bool) -> Result<Report, OpError> {
    let _lock = super::config_text::lock(&ctx.home);
    let file = ctx.home.config_file();
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(fail(format!("{}: {e}", file.display()))),
    };
    let (edited, changed) = config_text::edit_top(&text, "allow_private_endpoints", on.then_some("true"));
    if changed {
        config_text::write_checked(&ctx.home, &edited).map_err(fail)?;
    }
    let word = if on { "on" } else { "off" };
    Ok(finish(ctx, log, &format!("private endpoints {word}"), json!({"private_endpoints": word})))
}

pub fn private_endpoints(ctx: &Ctx, on: bool) -> Result<Done, OpError> {
    private_endpoints_with(ctx, &mut Vec::new(), on).map(|r| r.done)
}

/// Writes `plugins/<id>.toml` for a custom compatible provider, after the same gate and fit
/// check a user plugin gets, and reloads. `style` is `openai` or `anthropic` (or the full wire
/// name); `name` defaults to the id.
pub fn add_compatible_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    id: &str,
    name: Option<&str>,
    style: &str,
    base_url: &str,
    models: &[String],
) -> Result<Report, OpError> {
    let style = Style::parse(style)
        .ok_or_else(|| in_field("style", 1, format!("unknown style {style:?}; allowed: openai, anthropic")))?;
    let spec = Spec { id, name: name.unwrap_or_default(), style, base_url, models };
    let src = compatible::build(&spec).map_err(|e| in_field(e.field, 1, e.text))?;

    let taken = ctx.home.plugins_dir().join(format!("{id}.toml"));
    let known = crate::views::open_registry(&ctx.home)
        .map_err(|e| OpError { text: e.message, exit: e.code, field: None })?
        .snapshot()
        .provider(id)
        .is_ok();
    if known || taken.exists() {
        return Err(in_field("id", 1, format!("{id}: a provider with this id exists; choose another id")));
    }

    let allow_private = load_config(&ctx.home).map_err(fail)?.allow_private_endpoints;
    if !allow_private
        && validate::check_endpoint_url(base_url, false).is_err()
        && validate::check_endpoint_url(base_url, true).is_ok()
    {
        return Err(in_field(
            "base_url",
            REFUSED,
            format!(
                "{id}: base_url {} is a private address; turn private endpoints on first (nullrouter plugins private-endpoints on)",
                compatible::redact(base_url)
            ),
        ));
    }
    match check_user_plugin(&src, &taken, allow_private) {
        Err(errors) => {
            let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
            return Err(in_field("base_url", 1, lines.join("\n")));
        }
        Ok(v) => {
            if let Some(m) = v.message(&taken.display().to_string()) {
                return Err(in_field("base_url", REFUSED, m));
            }
        }
    }

    let dir = ctx.home.plugins_dir();
    std::fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    // `create_new` on the final path: a racing post with the same id gets the "taken" refusal.
    // The file is written whole and synced before the lock drops; a reader of the plugins dir
    // that sees a half-written file only sees it for this write.
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let made = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&taken);
        match made {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(in_field("id", 1, format!("{id}: a provider with this id exists; choose another id")));
            }
            Err(e) => return Err(fail(format!("{}: {e}", taken.display()))),
            Ok(mut f) => {
                if let Err(e) = f.write_all(src.as_bytes()).and_then(|()| f.sync_all()) {
                    let _ = std::fs::remove_file(&taken);
                    return Err(fail(format!("{}: {e}", taken.display())));
                }
            }
        }
    }

    let n = models.len();
    let plural = if n == 1 { "model" } else { "models" };
    Ok(finish(ctx, log, &format!("{id}: added ({n} {plural})"), json!({"id": id, "result": "added", "models": n})))
}

pub fn add_compatible(
    ctx: &Ctx,
    id: &str,
    name: Option<&str>,
    style: &str,
    base_url: &str,
    models: &[String],
) -> Result<Done, OpError> {
    add_compatible_with(ctx, &mut Vec::new(), id, name, style, base_url, models).map(|r| r.done)
}

/// Which visibility switch a provider op turns.
#[derive(Clone, Copy)]
enum Switch {
    Enable,
    Disable,
    Hide,
    Show,
}

impl Switch {
    /// The key, its value (`None` removes it: the default), and the line's word.
    fn parts(self) -> (&'static str, Option<&'static str>, &'static str) {
        match self {
            Self::Enable => ("enabled", None, "enabled"),
            Self::Disable => ("enabled", Some("false"), "disabled (routing skips it)"),
            Self::Hide => ("hidden", Some("true"), "hidden"),
            Self::Show => ("hidden", None, "shown"),
        }
    }
}

fn visibility_with(ctx: &Ctx, log: &mut Vec<String>, id: &str, switch: Switch) -> Result<Report, OpError> {
    let known = crate::views::open_registry(&ctx.home)
        .map_err(|e| OpError { text: e.message, exit: e.code, field: None })?
        .snapshot()
        .provider(id)
        .is_ok();
    if !known {
        return Err(in_field("id", 1, format!("{id}: no such provider")));
    }
    let _lock = super::config_text::lock(&ctx.home);
    let file = ctx.home.config_file();
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(fail(format!("{}: {e}", file.display()))),
    };
    let (key, value, word) = switch.parts();
    let (edited, changed) = config_text::edit(&text, &format!("provider.{id}"), key, value);
    if changed {
        config_text::write_checked(&ctx.home, &edited).map_err(fail)?;
    }
    Ok(finish(ctx, log, &format!("{id}: {word}"), json!({"id": id, "result": word})))
}

macro_rules! visibility_ops {
    ($($name:ident, $with:ident, $switch:ident;)*) => {$(
        pub fn $with(ctx: &Ctx, log: &mut Vec<String>, id: &str) -> Result<Report, OpError> {
            visibility_with(ctx, log, id, Switch::$switch)
        }

        pub fn $name(ctx: &Ctx, id: &str) -> Result<Done, OpError> {
            $with(ctx, &mut Vec::new(), id).map(|r| r.done)
        }
    )*};
}

visibility_ops! {
    enable, enable_with, Enable;
    disable, disable_with, Disable;
    hide, hide_with, Hide;
    show, show_with, Show;
}

#[cfg(test)]
mod tests {
    use super::*;
    use nullrouter_registry::OperatorHome;

    fn ctx(dir: &tempfile::TempDir) -> Ctx {
        Ctx { home: OperatorHome::new(dir.path().to_owned()), engine: None }
    }

    fn m() -> Vec<String> {
        vec!["a".into(), "b".into(), "c".into()]
    }

    #[test]
    fn a_compatible_provider_is_written_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(&dir);
        let r = add_compatible(&c, "local-llm", Some("Local"), "openai", "https://llm.example.com/v1", &m()).unwrap();
        assert_eq!(r.text, "local-llm: added (3 models), saved; applies at next start");
        assert!(dir.path().join("plugins/local-llm.toml").exists());
        let e = add_compatible(&c, "local-llm", None, "openai", "https://llm.example.com/v1", &m()).unwrap_err();
        assert_eq!((e.field, e.exit), (Some("id"), 1));
        let e = add_compatible(&c, "anthropic", None, "openai", "https://llm.example.com/v1", &m()).unwrap_err();
        assert_eq!(e.field, Some("id"));
    }

    #[test]
    fn a_private_address_needs_the_setting() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(&dir);
        let url = "http://127.0.0.1:11434";
        let e = add_compatible(&c, "local-llm", None, "anthropic", url, &m()).unwrap_err();
        assert_eq!(e.exit, 3);
        assert_eq!(
            e.text,
            "local-llm: base_url http://127.0.0.1:11434 is a private address; turn private endpoints on first (nullrouter plugins private-endpoints on)"
        );
        assert!(!dir.path().join("plugins/local-llm.toml").exists());
        private_endpoints(&c, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
            "allow_private_endpoints = true\n"
        );
        add_compatible(&c, "local-llm", None, "anthropic", url, &m()).unwrap();
        // Off would leave a plugin the loader then refuses: the loader's check refuses the edit.
        let e = private_endpoints(&c, false).unwrap_err();
        assert!(e.text.contains("allow_private_endpoints"), "{}", e.text);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
            "allow_private_endpoints = true\n"
        );
    }

    #[test]
    fn visibility_edits_the_provider_table_and_refuses_unknown_ids() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(&dir);
        let cfg = dir.path().join("config.toml");
        let d = disable(&c, "anthropic").unwrap();
        assert_eq!(d.text, "anthropic: disabled (routing skips it), saved; applies at next start");
        hide(&c, "anthropic").unwrap();
        let t = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            t.contains("[provider.anthropic]") && t.contains("enabled = false") && t.contains("hidden = true"),
            "{t}"
        );
        enable(&c, "anthropic").unwrap();
        show(&c, "anthropic").unwrap();
        assert!(!std::fs::read_to_string(&cfg).unwrap().contains("provider.anthropic"));
        let e = hide(&c, "nope").unwrap_err();
        assert_eq!(e.field, Some("id"));
    }

    #[test]
    fn a_bad_url_or_style_names_its_field() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(&dir);
        let e = add_compatible(&c, "x", None, "openai", "nope", &m()).unwrap_err();
        assert_eq!((e.field, e.exit), (Some("base_url"), 1));
        let e = add_compatible(&c, "x", None, "gemini", "https://a.b", &m()).unwrap_err();
        assert_eq!(e.field, Some("style"));
    }
}
