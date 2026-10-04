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

/// A schema-2 user plugin, as a community author would write one.
const ACME: &str = r#"schema = 2
id = "acme"
category = "apikey"

[auth]
kind = "apikey"
header = "Authorization"
scheme = "bearer"

[endpoints.text]
url = "https://api.acme.example/v1/chat/completions"
wire = "openai-chat"

[[models]]
id = "m1"
"#;

const SIGNIN: &str = r#"
[signin]
flow = "device_code"
client_id = "b1a00492-073a-47ea-816f-4c329264a828"
device_url = "https://auth.acme.example/oauth2/device/code"
token_url = "https://auth.acme.example/oauth2/token"
refresh_lead = "5m"
"#;

const IDENTITY: &str = "\n[identity.headers]\nUser-Agent = \"acme-cli/1.0\"\nx-acme-req = \"{request.id}\"\n";

const QUOTA: &str = r#"
[quota]
accounts = "any"
request = { url = "https://api.acme.example/v1/usage" }

[[quota.window]]
path = "limits[*]"
name = "{name|lower}"
unit = "requests"
used = "used"
"#;

const MODELS_LIVE: &str =
    "\n[models_live]\nurl = \"https://api.acme.example/v1/models\"\nlist = \"data\"\nid = \"id\"\n";

/// US6, FR-029: in this slice `[signin]`, `[identity]`, `[quota]` and `[models_live]` are
/// bundled-only. A user (community) plugin carrying any of them passes the gate, is refused
/// whole naming the part and why, and contributes nothing; the same file fits as bundled.
#[test]
fn a_community_plugin_with_a_sign_in_or_quota_section_is_refused_whole() {
    use nullrouter_registry::validate::validate_with;
    use nullrouter_registry::{PluginSource, bundled_gate_ctx, fit};

    let sign_in = "account sign-in is not supported";
    // `(case, source, the refused parts as (path, reason))`.
    type Case<'a> = (&'a str, String, &'a [(&'a str, &'a str)]);
    let cases: [Case; 5] = [
        ("signin", format!("{ACME}{SIGNIN}"), &[("signin", sign_in)]),
        ("identity", format!("{ACME}{SIGNIN}{IDENTITY}"), &[("signin", sign_in), ("identity", sign_in)]),
        ("models_live", format!("{ACME}{SIGNIN}{MODELS_LIVE}"), &[("signin", sign_in), ("models_live", sign_in)]),
        ("quota", format!("{ACME}{QUOTA}"), &[("quota", "quota is not supported")]),
        (
            "all",
            format!("{ACME}{SIGNIN}{IDENTITY}{QUOTA}{MODELS_LIVE}"),
            &[
                ("signin", sign_in),
                ("identity", sign_in),
                ("models_live", sign_in),
                ("quota", "quota is not supported"),
            ],
        ),
    ];
    for (case, src, want) in cases {
        let file = format!("{case}.toml");
        let verdict = check_user_plugin(&src, Path::new(&file), false).unwrap_or_else(|e| panic!("{case}: {e:#?}"));
        let FitVerdict::Unsupported { parts } = &verdict else { panic!("{case}: a community plugin fits") };
        let got: Vec<(String, &str)> = parts.iter().map(|p| (p.path.to_string(), p.reason.as_str())).collect();
        let want: Vec<(String, &str)> = want.iter().map(|(k, r)| ((*k).to_owned(), *r)).collect();
        assert_eq!(got, want, "{case}");
        let message = verdict.message(&file).unwrap();
        assert!(message.starts_with(&format!("{file}: not supported by this core")), "{message}");
        assert!(message.ends_with("No part of this plugin was loaded."), "{message}");
        assert!(parts.iter().all(|p| p.line > 0 && p.col > 0), "{case}: {parts:?}");

        // The rule is the source alone: bundled, the same file fits.
        let ctx = bundled_gate_ctx(true, false).unwrap();
        let bundled = validate_with(&src, PluginSource::Bundled, &file, &ctx).unwrap().entity;
        assert_eq!(fit::check(&bundled, &src, &file, &ctx), FitVerdict::Fits, "{case}");

        // Installed by hand, it contributes nothing.
        let home = home();
        let path = home.path().join("plugins/acme.toml");
        fs::write(&path, &src).unwrap();
        let reg = RegistryHandle::open(OperatorHome::new(home.path())).unwrap().snapshot();
        assert!(reg.provider("acme").is_err(), "{case}");
        assert!(reg.resolve("acme/m1").is_err(), "{case}");
        let r = reg.report();
        assert_eq!(r.unsupported.len(), 1, "{case}: {r:#?}");
        assert_eq!(r.unsupported[0].id, "acme");
        let named = if case == "quota" { "quota is not supported" } else { sign_in };
        assert!(r.unsupported[0].message.contains(named), "{}", r.unsupported[0].message);
    }
}

/// FR-029: the generated community set declares none of the slice 005 sections, so it
/// loads or is refused exactly as before; xai and grok-cli left it for the bundle.
#[test]
fn the_community_set_carries_no_sign_in_or_quota_section() {
    let set = community::community();
    for p in set {
        let doc: toml::Table = toml::from_str(p.src).unwrap();
        for key in ["signin", "identity", "quota", "models_live"] {
            assert!(!doc.contains_key(key), "{}: declares [{key}]", p.file());
        }
    }
    assert!(set.iter().all(|p| p.id != "xai" && p.id != "grok-cli"));
    let refused = set.iter().filter(|p| !p.verdict.as_ref().unwrap().fits()).count();
    assert_eq!((set.len() - refused, refused), FIT_REFUSED, "fit, refused");
}
/// Fitting and refused community plugins since xai and grok-cli moved to the bundle.
const FIT_REFUSED: (usize, usize) = (33, 81);

/// Slice 006: `[routing]` holds no URL, header or secret, so a community plugin may declare it
/// and still fit; it loads with its meters and price schedule.
#[test]
fn a_community_plugin_may_declare_routing() {
    let src = r#"
schema = 2
id = "acme"
category = "apikey"

[auth]
kind = "apikey"
header = "x-acme-key"

[endpoints.text]
url = "https://api.acme.example/v1/messages"
wire = "anthropic-messages"

[routing.cache]
mode = "explicit"
lifetime = "1h"

[[routing.window]]
name = "daily"
length = "1d"
unit = "requests"
capacity = 1500
reset = "fixed"
anchor = "00:00+00:00"

[[routing.price]]
when = { days = ["sat", "sun"], from = "00:00", to = "12:00" }
input = 0.5

[[routing.price]]
input = 1.0
"#;
    let home = home();
    let path = home.path().join("plugins/acme.toml");
    let verdict = check_user_plugin(src, &path, false).unwrap_or_else(|e| panic!("{e:#?}"));
    assert!(verdict.fits(), "{verdict:?}");
    fs::write(&path, src).unwrap();
    let reg = RegistryHandle::open(OperatorHome::new(home.path())).unwrap().snapshot();
    let r = reg.provider("acme").unwrap().routing();
    assert_eq!(r.cache.lifetime.as_secs(), 3600);
    assert_eq!((r.windows.len(), r.prices.len()), (1, 2));
}
