//! US8, SC-012: the community set installs whole or is refused whole, with a message naming
//! every unsupported part (research R19).

use std::fs;
use std::path::{Path, PathBuf};

use nullrouter_registry::community::{self, InstallError};
use nullrouter_registry::fit::FitVerdict;
use nullrouter_registry::schema::ModelType;
use nullrouter_registry::{OperatorHome, RegistryHandle, Resolution, check_user_plugin};

fn home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join("plugins")).unwrap();
    home
}

/// Every community plugin either fits and loads, or is refused with each part positioned
/// in its file.
#[test]
fn every_community_plugin_fits_and_loads_or_is_refused_whole() {
    let set = community::community();
    assert_eq!(set.len(), 114);
    let (mut fits, mut refused) = (0, 0);
    for p in set {
        let file = p.file();
        match p.verdict.as_ref().unwrap_or_else(|e| panic!("{file}: invalid: {e:#?}")) {
            FitVerdict::Fits => {
                fits += 1;
                let home = home();
                fs::write(home.path().join("plugins").join(format!("{}.toml", p.id)), p.src).unwrap();
                // Self-hosted plugins point at private addresses.
                fs::write(home.path().join("config.toml"), "allow_private_endpoints = true\n").unwrap();
                let reg = RegistryHandle::open(OperatorHome::new(home.path())).unwrap().snapshot();
                let r = reg.report();
                assert!(r.unsupported.is_empty() && r.skipped.is_empty(), "{}: {r:#?}", p.id);
                let entity = reg.provider(p.id).unwrap_or_else(|e| panic!("{}: {e:?}", p.id));
                assert!(!entity.endpoints.is_empty(), "{}: fits but converts to no endpoint", p.id);
            }
            v @ FitVerdict::Unsupported { parts } => {
                refused += 1;
                assert!(!parts.is_empty(), "{file}");
                let message = v.message(&file).unwrap();
                assert!(message.starts_with(&format!("{file}: not supported by this core (0router ")), "{message}");
                assert!(message.ends_with("\nNo part of this plugin was loaded."), "{message}");
                for part in parts {
                    assert!(part.line > 0 && part.col > 0, "{file}: unpositioned {part}");
                    assert!(message.contains(&format!("  - {part}\n")), "{message}");
                }
            }
        }
    }
    assert_eq!(fits + refused, 114);
    assert!(fits > 0 && refused > 0, "{fits} fit, {refused} refused");
}

/// A refused plugin contributes nothing: no provider, no tokens, no unified-model member.
#[test]
fn a_refused_plugin_contributes_nothing() {
    let qoder = community::community().iter().find(|p| p.id == "qoder").unwrap();
    assert!(!qoder.verdict.as_ref().unwrap().fits());
    let home = home();
    let path = home.path().join("plugins/qoder.toml");
    fs::write(&path, qoder.src).unwrap();
    fs::write(
        home.path().join("config.toml"),
        "[[unified_model]]\nname = \"needs-qoder\"\nmembers = [{ provider = \"qoder\", model = \"m\" }]\n",
    )
    .unwrap();
    let reg = RegistryHandle::open(OperatorHome::new(home.path())).unwrap().snapshot();
    assert!(reg.provider("qoder").is_err());
    assert!(reg.providers().all(|p| p.id != "qoder"));
    assert!(reg.resolve("qoder/anything").is_err());
    assert!(reg.unified_model("needs-qoder").is_err());
    let r = reg.report();
    assert_eq!(r.unsupported.len(), 1);
    assert_eq!((r.unsupported[0].path.as_path(), r.unsupported[0].id.as_str()), (path.as_path(), "qoder"));
    assert!(r.unsupported[0].message.contains("needs a provider-specific executor"), "{}", r.unsupported[0].message);
    assert_eq!(r.dropped_unified_models.len(), 1);
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/unsupported")
}

/// T126: each case's refusal matches its golden `.expected` message. `NR_BLESS=1` rewrites
/// the goldens.
#[test]
fn unsupported_corpus_matches_the_goldens() {
    let mut cases: Vec<PathBuf> = fs::read_dir(corpus_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    cases.sort();
    assert_eq!(cases.len(), 13);
    for path in cases {
        let name = path.file_name().unwrap().to_str().unwrap();
        let shown = Path::new("unsupported").join(name);
        let src = fs::read_to_string(&path).unwrap();
        let verdict = check_user_plugin(&src, &shown, false).unwrap_or_else(|e| panic!("{name}: invalid: {e:#?}"));
        let got = verdict.message(&shown.display().to_string()).unwrap_or_else(|| panic!("{name}: fits"));
        let golden = path.with_extension("expected");
        if std::env::var_os("NR_BLESS").is_some() {
            fs::write(&golden, format!("{got}\n")).unwrap();
        }
        let want = fs::read_to_string(&golden).unwrap_or_else(|e| panic!("{}: {e}", golden.display()));
        assert_eq!(got, want.trim_end_matches('\n'), "{name}");
    }
    let mixed = fs::read_to_string(corpus_dir().join("mixed.expected")).unwrap();
    assert!(mixed.lines().filter(|l| l.starts_with("  - ")).count() >= 3, "{mixed}");
}

/// `install` copies a fitting plugin, converts it on load, and `uninstall` removes it.
#[test]
fn install_and_uninstall() {
    let home = home();
    let operator = OperatorHome::new(home.path());
    match community::install("qoder", &operator) {
        Err(InstallError::Unsupported(m)) => assert!(m.ends_with("No part of this plugin was loaded."), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(!community::is_installed("qoder", &operator));
    assert!(matches!(community::install("nope", &operator), Err(InstallError::UnknownId(_))));

    let dest = community::install("groq", &operator).unwrap();
    assert_eq!(dest, home.path().join("plugins/groq.toml"));
    assert!(matches!(community::install("groq", &operator), Err(InstallError::AlreadyInstalled(_))));
    let reg = RegistryHandle::open(OperatorHome::new(home.path())).unwrap().snapshot();
    let groq = reg.provider("groq").unwrap();
    assert!(!groq.is_bundled());
    let text = reg.endpoints("groq", ModelType::Text);
    assert_eq!(text.len(), 1);
    assert_eq!(
        (text[0].url.as_str(), text[0].wire.as_deref()),
        ("https://api.groq.com/openai/v1/chat/completions", Some("openai-chat"))
    );
    let model = &groq.models.as_ref().unwrap()[0].id;
    assert!(matches!(reg.resolve(&format!("groq/{model}")), Ok(Resolution::Direct { .. })));

    community::uninstall("groq", &operator).unwrap();
    assert!(!dest.exists());
    assert!(matches!(community::uninstall("groq", &operator), Err(InstallError::NotInstalled(_))));
}
