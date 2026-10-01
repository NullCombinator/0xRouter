//! US4 s5–s7 and the startup/reload rules for user plugins (FR-010, FR-012a, FR-013, FR-024).

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use zerorouter_registry::{OperatorHome, RegistryHandle, Resolution, ResolvedCredential, StartupError};

struct Home(TempDir);

impl Home {
    fn new() -> Self {
        let home = Self(tempfile::tempdir().unwrap());
        fs::create_dir(home.plugins()).unwrap();
        home
    }

    fn plugins(&self) -> PathBuf {
        self.0.path().join("plugins")
    }

    fn plugin(&self, file: &str, src: &str) -> PathBuf {
        let path = self.plugins().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, src).unwrap();
        path
    }

    fn config(&self, src: &str) {
        fs::write(self.0.path().join("config.toml"), src).unwrap();
    }

    fn open(&self) -> Result<RegistryHandle, StartupError> {
        RegistryHandle::open_parity(OperatorHome::new(self.0.path()))
    }
}

/// A plugin of the parity set: bundled, or community (loaded as bundled there).
fn bundled(file: &str) -> &'static str {
    let mut all = zerorouter_registry::bundled_sources().iter().chain(zerorouter_registry::community::COMMUNITY);
    all.find(|(f, _)| *f == file).unwrap().1
}

const EXTRA: &str = "id = \"extra\"\ncategory = \"apikey\"\nmodels = [\"e-1\"]\n";

#[test]
fn s5_bundled_conflict_follows_the_decision() {
    let home = Home::new();
    let path = home.plugin("kiro.toml", bundled("kiro.toml"));

    let reg = home.open().unwrap().snapshot();
    assert!(reg.provider("kiro").unwrap().is_bundled());
    let r = reg.report();
    assert_eq!(r.pending_conflicts.len(), 1);
    assert_eq!((r.pending_conflicts[0].id.as_str(), r.pending_conflicts[0].path.as_path()), ("kiro", path.as_path()));
    assert_eq!(r.user, 0);

    home.config("[plugin_decisions]\nkiro = \"replace\"\n");
    let reg = home.open().unwrap().snapshot();
    assert!(!reg.provider("kr").unwrap().is_bundled());
    assert!(reg.report().pending_conflicts.is_empty());
    assert_eq!((reg.report().bundled, reg.report().user), (120, 1));

    home.config("[plugin_decisions]\nkiro = \"decline\"\n");
    let reg = home.open().unwrap().snapshot();
    assert!(reg.provider("kiro").unwrap().is_bundled());
    assert_eq!(reg.report().declined.len(), 1);
    assert_eq!(reg.report().declined[0].path, path);
}

#[test]
fn s6_credentials_are_bound_to_hosts() {
    let home = Home::new();
    home.plugin("gemini-cli.toml", bundled("gemini-cli.toml"));
    home.config(
        "[plugin_decisions]\ngemini-cli = \"replace\"\n\n[provider.gemini-cli]\nallow_uncatalogued_models = false\n",
    );
    let reg = home.open().unwrap().snapshot();
    assert!(!reg.provider("gemini-cli").unwrap().is_bundled());
    assert!(matches!(reg.credential("gemini-cli"), Some(ResolvedCredential::Available(_))));
    assert!(reg.report().withheld_credentials.is_empty());
    // Settings survive a replace.
    assert!(!reg.settings("gemini-cli").allow_uncatalogued_models);

    home.plugin("gemini-cli.toml", &bundled("gemini-cli.toml").replace("oauth2.googleapis.com", "evil.example"));
    let reg = home.open().unwrap().snapshot();
    let Some(ResolvedCredential::Withheld { offending_url }) = reg.credential("gemini-cli") else {
        panic!("credential released to evil.example")
    };
    assert_eq!(offending_url.host_str(), Some("evil.example"));
    assert!(reg.composed_transport("gemini-cli").unwrap().client_secret.is_none());
    let withheld = &reg.report().withheld_credentials;
    assert_eq!(withheld.len(), 1);
    let msg = withheld[0].to_string();
    assert!(msg.contains("gemini-cli") && msg.contains("evil.example"), "{msg}");
}

#[test]
fn s7_duplicate_ids_and_claimed_tokens() {
    let home = Home::new();
    let a = home.plugin("a.toml", "id = \"dup\"\ncategory = \"apikey\"\n");
    let b = home.plugin("b.toml", "id = \"dup\"\ncategory = \"apikey\"\n");
    home.plugin("extra.toml", EXTRA);
    let reg = home.open().unwrap().snapshot();
    assert!(reg.provider("dup").is_err());
    let skipped = &reg.report().skipped;
    assert_eq!(skipped.iter().map(|s| s.path.as_path()).collect::<Vec<_>>(), [a.as_path(), b.as_path()]);
    for s in skipped {
        let e = s.errors[0].to_string();
        assert!(e.contains(&a.display().to_string()) && e.contains(&b.display().to_string()), "{e}");
    }
    assert!(reg.provider("extra").is_ok());

    let home = Home::new();
    let path = home.plugin("thief.toml", "id = \"thief\"\ncategory = \"apikey\"\nalias = \"kr\"\n");
    let reg = home.open().unwrap().snapshot();
    assert_eq!(reg.provider("kr").unwrap().id, "kiro");
    let skipped = &reg.report().skipped;
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].path, path);
    let e = skipped[0].errors[0].to_string();
    assert!(e.contains("\"kr\"") && e.contains("kiro (bundled)") && e.contains("thief"), "{e}");
}

#[test]
fn invalid_user_plugin_is_skipped_at_startup() {
    let home = Home::new();
    let bad = home.plugin("broken.toml", "id = \"broken\"\ncategory = \"local\"\n");
    home.plugin("extra.toml", EXTRA);
    home.plugin("sub/nested.toml", "id = \"nested\"\ncategory = \"apikey\"\n");
    home.plugin("notes.txt", "not a plugin");
    let reg = home.open().unwrap().snapshot();
    assert!(reg.provider("extra").is_ok());
    assert!(reg.provider("nested").is_err(), "subdirectories are not scanned");
    let r = reg.report();
    assert_eq!((r.bundled, r.user), (121, 1));
    assert_eq!(r.skipped.len(), 1);
    assert_eq!(r.skipped[0].path, bad);
    assert!(r.skipped[0].errors[0].to_string().contains("category"));
}

#[test]
fn unified_model_on_a_skipped_plugin_is_dropped_at_startup() {
    let home = Home::new();
    home.plugin("broken.toml", "id = \"broken\"\ncategory = \"local\"\n");
    home.config(
        r#"
[[unified_model]]
name = "needs-broken"
members = [{ provider = "broken", model = "m" }]

[[unified_model]]
name = "fine"
members = [{ provider = "kr", model = "claude-sonnet-4.5" }]
"#,
    );
    let reg = home.open().unwrap().snapshot();
    assert!(reg.unified_model("needs-broken").is_err());
    assert!(matches!(reg.resolve("fine"), Ok(Resolution::Unified(_))));
    let dropped = &reg.report().dropped_unified_models;
    assert_eq!(dropped.len(), 1);
    assert_eq!((dropped[0].name.as_str(), dropped[0].provider.as_str()), ("needs-broken", "broken"));
}

#[test]
fn reload_rejects_an_invalid_user_plugin() {
    let home = Home::new();
    home.plugin("extra.toml", EXTRA);
    let handle = home.open().unwrap();
    home.plugin("broken.toml", "id = \"broken\"\ncategory = \"local\"\n");
    let err = handle.reload().unwrap_err();
    assert!(err.to_string().contains("broken.toml"), "{err}");
    assert!(handle.snapshot().provider("extra").is_ok());
    assert_eq!(handle.snapshot().report().user, 1);
}

#[test]
fn reload_rejects_removing_a_referenced_plugin() {
    let home = Home::new();
    let extra = home.plugin("extra.toml", EXTRA);
    home.config("[[unified_model]]\nname = \"ex\"\nmembers = [{ provider = \"extra\", model = \"e-1\" }]\n");
    let handle = home.open().unwrap();
    fs::remove_file(&extra).unwrap();
    let err = handle.reload().unwrap_err().to_string();
    assert!(err.contains("unified_model[0].members[0].provider") && err.contains("\"extra\""), "{err}");
    assert!(matches!(handle.snapshot().resolve("ex"), Ok(Resolution::Unified(_))));
}

#[test]
fn plugin_file_path_is_reported_as_given() {
    let home = Home::new();
    let path = home.plugin("extra.toml", EXTRA);
    let reg = home.open().unwrap().snapshot();
    let p = reg.provider("extra").unwrap();
    assert!(!p.is_bundled());
    assert_eq!(p.source, zerorouter_registry::PluginSource::User(path.clone()));
    assert!(Path::new(&path).exists());
}
