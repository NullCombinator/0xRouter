//! Loading a snapshot: bundled plugins, user plugins, and `config.toml`
//! (contracts/operator-config.md, data-model § Lifecycle).
//!
//! Startup and reload run the same pipeline. They differ only in what an error does:
//! at startup an invalid *user* plugin is skipped and reported (FR-010); on reload any
//! error rejects the whole candidate (FR-024).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::{env, fs, io};

use url::Url;

use crate::registry::{Registry, UnifiedModel, UnifiedMember, token_clashes, token_path};
use crate::schema::{Decision, OperatorConfig, PluginSource, ProviderEntity, ProviderSettings};
use crate::validate::gate::{parse, positioned};
use crate::validate::{FieldPath, ValidationError, validate};

include!(concat!(env!("OUT_DIR"), "/bundled_plugins.rs"));

/// The operator's directory: `config.toml` and `plugins/*.toml`. Missing parts are not errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorHome(PathBuf);

impl OperatorHome {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// `$ZEROROUTER_HOME`, else `~/.0router`.
    pub fn resolve() -> Self {
        match env::var_os("ZEROROUTER_HOME").filter(|v| !v.is_empty()) {
            Some(home) => Self(home.into()),
            None => Self(env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from).join(".0router")),
        }
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn config_file(&self) -> PathBuf {
        self.0.join("config.toml")
    }

    pub fn plugins_dir(&self) -> PathBuf {
        self.0.join("plugins")
    }
}

/// What a load did besides succeeding: everything the operator should look at.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadReport {
    pub bundled: usize,
    pub user: usize,
    pub unified_models: usize,
    /// A user plugin shadows a bundled id and `config.toml` has no decision yet.
    pub pending_conflicts: Vec<PluginConflict>,
    pub declined: Vec<PluginConflict>,
    pub withheld_credentials: Vec<WithheldCredential>,
    /// User plugins skipped at startup, with their errors.
    pub skipped: Vec<SkippedPlugin>,
    /// Unified models dropped at startup because a member's plugin was skipped.
    pub dropped_unified_models: Vec<DroppedUnifiedModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginConflict {
    pub id: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithheldCredential {
    pub provider: String,
    pub offending_url: Url,
}

impl fmt::Display for WithheldCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OAuth for {} will not work: the bundled client secret is withheld because the active plugin sends OAuth traffic to {}",
            self.provider, self.offending_url
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPlugin {
    pub path: PathBuf,
    /// The declared id if the file parsed that far, else the file stem.
    pub id: String,
    pub errors: Vec<ValidationError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedUnifiedModel {
    pub name: String,
    pub provider: String,
}

fn render(errors: &[ValidationError]) -> String {
    errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
}

/// Startup failed: an invalid bundled plugin or an invalid `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", render(.errors))]
pub struct StartupError {
    pub errors: Vec<ValidationError>,
}

/// Reload rejected. The previous snapshot is still active.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", render(.errors))]
pub struct ReloadError {
    pub errors: Vec<ValidationError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Startup,
    Reload,
}

/// A validated plugin plus the text it came from, so later checks can position errors.
struct Loaded {
    entity: ProviderEntity,
    src: Cow<'static, str>,
    file: String,
}

impl Loaded {
    fn error(&self, path: FieldPath, rule: String) -> ValidationError {
        positioned(&self.src, &self.file, path, rule)
    }

    fn user_path(&self) -> Option<&Path> {
        match &self.entity.source {
            PluginSource::User(p) => Some(p),
            PluginSource::Bundled => None,
        }
    }

    fn describe(&self) -> String {
        match self.user_path() {
            Some(p) => format!("{} ({})", self.entity.id, p.display()),
            None => format!("{} (bundled)", self.entity.id),
        }
    }
}

/// Every embedded plugin through the gate. Any error is fatal.
#[cfg(test)]
pub(crate) fn load_bundled() -> Result<Vec<ProviderEntity>, Vec<ValidationError>> {
    bundled().map(|v| v.into_iter().map(|l| l.entity).collect())
}

fn bundled() -> Result<Vec<Loaded>, Vec<ValidationError>> {
    let mut out = Vec::with_capacity(BUNDLED.len());
    let mut errors = Vec::new();
    for (name, src) in BUNDLED {
        let file = format!("plugins/bundled/{name}");
        match validate(src, PluginSource::Bundled, &file) {
            Ok(entity) => out.push(Loaded { entity, src: Cow::Borrowed(src), file }),
            Err(e) => errors.extend(e),
        }
    }
    if errors.is_empty() { Ok(out) } else { Err(errors) }
}

/// Builds a full snapshot from `home`.
pub(crate) fn build(home: &OperatorHome, mode: Mode) -> Result<Registry, Vec<ValidationError>> {
    let bundled = bundled()?;
    let config_file = home.config_file();
    let config_name = config_file.display().to_string();
    let config_src = read_optional(&config_file).map_err(|e| vec![io_error(&config_file, &e)])?;
    let config: OperatorConfig = match &config_src {
        Some(src) => parse(src, &config_name)?,
        None => OperatorConfig::default(),
    };
    let config_src = config_src.unwrap_or_default();

    let mut report = LoadReport::default();
    let mut errors = Vec::new();
    let skip = |report: &mut LoadReport, errors: &mut Vec<ValidationError>, l: &Loaded, e: Vec<ValidationError>| {
        match (mode, l.user_path()) {
            (Mode::Startup, Some(p)) => {
                report.skipped.push(SkippedPlugin { path: p.to_owned(), id: l.entity.id.clone(), errors: e })
            }
            _ => errors.extend(e),
        }
    };

    // User plugins, then duplicate user ids (both skipped / rejected).
    let mut user = discover(home, mode, &mut report, &mut errors);
    let mut by_id: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, u) in user.iter().enumerate() {
        by_id.entry(&u.entity.id).or_default().push(i);
    }
    let mut dup: BTreeSet<usize> = BTreeSet::new();
    for idx in by_id.values().filter(|v| v.len() > 1) {
        let paths = idx.iter().map(|&i| user[i].file.as_str()).collect::<Vec<_>>().join(", ");
        for &i in idx {
            let e = user[i].error(FieldPath::of("id"), format!("id {:?} declared by more than one user plugin: {paths}", user[i].entity.id));
            skip(&mut report, &mut errors, &user[i], vec![e]);
            dup.insert(i);
        }
    }
    for i in dup.into_iter().rev() {
        user.remove(i);
    }

    // Conflicts with bundled ids (FR-013).
    let mut active = bundled;
    let slot: HashMap<String, usize> = active.iter().enumerate().map(|(i, l)| (l.entity.id.clone(), i)).collect();
    let bundled_ids: BTreeSet<String> = slot.keys().cloned().collect();
    for u in user {
        let Some(&i) = slot.get(&u.entity.id) else {
            active.push(u);
            continue;
        };
        let conflict = || PluginConflict { id: u.entity.id.clone(), path: u.user_path().unwrap_or(Path::new("")).to_owned() };
        match config.plugin_decisions.get(&u.entity.id) {
            None => report.pending_conflicts.push(conflict()),
            Some(Decision::Decline) => report.declined.push(conflict()),
            Some(Decision::Replace) => active[i] = u,
        }
    }

    // Tokens claimed twice. A user plugin involved is skipped at startup.
    let entities: Vec<&ProviderEntity> = active.iter().map(|l| &l.entity).collect();
    let mut drop: BTreeSet<usize> = BTreeSet::new();
    for c in token_clashes(&entities) {
        let (a, b) = (&active[c.first], &active[c.second]);
        let rule = format!("token {:?} is claimed by both {} and {}", c.token, a.describe(), b.describe());
        let blamed: Vec<usize> = [c.first, c.second].into_iter().filter(|&i| active[i].user_path().is_some()).collect();
        if blamed.is_empty() || mode == Mode::Reload {
            errors.push(b.error(token_path(&b.entity, &c.token), rule));
        } else {
            for i in blamed.into_iter().filter(|i| drop.insert(*i)) {
                let e = active[i].error(token_path(&active[i].entity, &c.token), rule.clone());
                skip(&mut report, &mut errors, &active[i], vec![e]);
            }
        }
    }
    for i in drop.into_iter().rev() {
        active.remove(i);
    }

    // `auth.credential_fallback` must name an active provider.
    let tokens: BTreeSet<&str> = active.iter().flat_map(|l| l.entity.tokens()).collect();
    let mut drop: BTreeSet<usize> = BTreeSet::new();
    for (i, l) in active.iter().enumerate() {
        if let Some(x) = l.entity.auth.as_ref().and_then(|a| a.credential_fallback.as_deref()) {
            if !tokens.contains(x) {
                let e = l.error(FieldPath::of("auth.credential_fallback"), format!("unknown provider {x:?}"));
                if mode == Mode::Startup && l.user_path().is_some() {
                    drop.insert(i);
                }
                skip(&mut report, &mut errors, l, vec![e]);
            }
        }
    }
    for i in drop.into_iter().rev() {
        active.remove(i);
    }

    report.user = active.iter().filter(|l| l.user_path().is_some()).count();
    report.bundled = active.len() - report.user;
    let skipped_ids: BTreeSet<String> = report.skipped.iter().map(|s| s.id.clone()).collect();

    let mut registry = Registry::new(active.into_iter().map(|l| l.entity).collect());
    let outcome = validate_config(&config, &config_src, &config_name, &registry, &bundled_ids, mode, &skipped_ids);
    errors.extend(outcome.errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    report.dropped_unified_models = outcome.dropped;
    report.unified_models = outcome.unified.len();
    report.withheld_credentials = registry.withheld_credentials();
    registry.set_operator_state(outcome.unified, outcome.settings, report);
    Ok(registry)
}

/// Top-level `*.toml` files in `plugins/`, sorted. Invalid files are skipped at startup.
fn discover(home: &OperatorHome, mode: Mode, report: &mut LoadReport, errors: &mut Vec<ValidationError>) -> Vec<Loaded> {
    let dir = home.plugins_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            errors.push(io_error(&dir, &e));
            return Vec::new();
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "toml"))
        .collect();
    paths.sort();

    let mut out = Vec::new();
    for path in paths {
        let file = path.display().to_string();
        let result = fs::read_to_string(&path)
            .map_err(|e| vec![io_error(&path, &e)])
            .and_then(|src| validate(&src, PluginSource::User(path.clone()), &file).map(|entity| (entity, src)));
        match result {
            Ok((entity, src)) => out.push(Loaded { entity, src: Cow::Owned(src), file }),
            Err(e) if mode == Mode::Startup => {
                let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                report.skipped.push(SkippedPlugin { path, id, errors: e });
            }
            Err(e) => errors.extend(e),
        }
    }
    out
}

fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn io_error(path: &Path, e: &io::Error) -> ValidationError {
    ValidationError { file: path.display().to_string(), line: 0, col: 0, path: FieldPath::root(), rule: e.to_string() }
}

pub(crate) struct ConfigOutcome {
    pub(crate) unified: Vec<UnifiedModel>,
    pub(crate) settings: BTreeMap<String, ProviderSettings>,
    pub(crate) errors: Vec<ValidationError>,
    pub(crate) dropped: Vec<DroppedUnifiedModel>,
}

/// Checks `config` against the candidate provider set `reg`. Startup and reload both
/// call this. At startup (only), a unified model whose member names a skipped user
/// plugin is dropped instead of failing the load.
pub(crate) fn validate_config(
    config: &OperatorConfig,
    src: &str,
    file: &str,
    reg: &Registry,
    bundled_ids: &BTreeSet<String>,
    mode: Mode,
    skipped: &BTreeSet<String>,
) -> ConfigOutcome {
    let mut found: Vec<(FieldPath, String)> = Vec::new();
    let mut err = |path: FieldPath, rule: String| found.push((path, rule));
    let excused = |token: &str| mode == Mode::Startup && skipped.contains(token);

    if let Some(v) = config.schema.filter(|v| *v != 1) {
        err(FieldPath::of("schema"), format!("unsupported schema version {v}; expected 1"));
    }

    let mut unified = Vec::new();
    let mut dropped = Vec::new();
    let mut names: HashMap<&str, usize> = HashMap::new();
    for (i, decl) in config.unified_model.iter().enumerate() {
        let base = FieldPath::of("unified_model").index(i);
        if decl.name.is_empty() || decl.name.contains('/') {
            err(base.key("name"), "must be non-empty and must not contain \"/\"".into());
        } else if let Some(&j) = names.get(decl.name.as_str()) {
            err(base.key("name"), format!("duplicate of unified_model[{j}]"));
        } else {
            names.insert(&decl.name, i);
        }
        if decl.members.is_empty() {
            err(base.key("members"), "must not be empty".into());
        }

        let mut members = Vec::with_capacity(decl.members.len());
        let mut seen: HashMap<&str, usize> = HashMap::new();
        let mut kind = decl.kind.map(|k| (k, None));
        let mut drop_for = None;
        for (j, m) in decl.members.iter().enumerate() {
            let mb = base.key("members").index(j);
            let Some(p) = reg.index_of(&m.provider) else {
                if excused(&m.provider) {
                    drop_for.get_or_insert_with(|| m.provider.clone());
                } else {
                    err(mb.key("provider"), format!("unknown provider {:?} (unified model {:?})", m.provider, decl.name));
                }
                continue;
            };
            let provider = &reg.providers[p];
            if seen.insert(&provider.id, j).is_some() {
                err(mb, format!("provider {:?} already a member", provider.id));
                continue;
            }
            let model = reg.find_at(p, &m.model);
            if model.is_none() && !provider.passthrough_models {
                err(mb.key("model"), format!("{:?} is not declared by provider {:?}", m.model, provider.id));
                continue;
            }
            if let Some(k) = model.and_then(|m| m.kind) {
                match kind {
                    None => kind = Some((k, Some(j))),
                    Some((want, _)) if want == k => {}
                    Some((want, None)) => err(mb, format!("kind \"{k}\" conflicts with unified_model kind \"{want}\"")),
                    Some((want, Some(first))) => {
                        err(mb, format!("kind \"{k}\" conflicts with members[{first}] kind \"{want}\""))
                    }
                }
            }
            members.push(UnifiedMember {
                provider: provider.id.clone(),
                requested: m.model.clone(),
                upstream_id: reg.upstream_at(p, &m.model),
                catalogued: model.is_some(),
            });
        }
        match drop_for {
            Some(provider) => dropped.push(DroppedUnifiedModel { name: decl.name.clone(), provider }),
            None => unified.push(UnifiedModel { name: decl.name.clone(), kind: decl.kind, members }),
        }
    }

    let mut settings = BTreeMap::new();
    for (token, s) in &config.provider {
        match reg.index_of(token) {
            Some(p) => {
                settings.insert(reg.providers[p].id.clone(), *s);
            }
            None if excused(token) => {}
            None => err(FieldPath::of("provider").key(token.as_str()), "unknown provider".into()),
        }
    }

    for id in config.plugin_decisions.keys().filter(|id| !bundled_ids.contains(*id)) {
        err(FieldPath::of("plugin_decisions").key(id.as_str()), "not a bundled provider id".into());
    }

    ConfigOutcome {
        unified,
        settings,
        errors: found.into_iter().map(|(path, rule)| positioned(src, file, path, rule)).collect(),
        dropped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_plugin_validates() {
        let providers = load_bundled().unwrap_or_else(|e| panic!("{}", render(&e)));
        assert_eq!(providers.len(), BUNDLED.len());
    }

    #[test]
    fn missing_home_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let reg = build(&OperatorHome::new(dir.path().join("absent")), Mode::Startup).unwrap();
        assert_eq!(reg.report().user, 0);
        assert_eq!(reg.unified_models().count(), 0);
    }
}
