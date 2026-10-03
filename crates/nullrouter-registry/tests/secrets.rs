//! US1 scenario 4, SC-002: bundled client secrets live in the core, never in plugin files,
//! and never print.

use std::path::PathBuf;

use nullrouter_registry::{OperatorHome, RegistryHandle, bundled_sources};
use serde_json::Value;

/// `(provider id, clientSecret)` for every fixture entry that has one.
fn fixture_secrets() -> Vec<(String, String)> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/9router/providers.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    doc["data"]
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(id, t)| Some((id.clone(), t.get("clientSecret")?.as_str()?.to_owned())))
        .collect()
}

#[test]
fn no_plugin_file_carries_a_secret() {
    let secrets = fixture_secrets();
    assert_eq!(secrets.len(), 4);
    let all: Vec<_> = bundled_sources().iter().chain(nullrouter_registry::community::COMMUNITY).collect();
    assert_eq!(all.len(), 121);
    for (file, src) in all {
        assert!(!src.contains("client_secret"), "{file} declares client_secret");
        for (id, secret) in &secrets {
            assert!(!src.contains(secret.as_str()), "{file} contains {id}'s client secret");
        }
    }
}

#[test]
fn bundled_secrets_are_released_and_opaque() {
    let home = tempfile::tempdir().unwrap();
    let reg = RegistryHandle::open_parity(OperatorHome::new(home.path())).unwrap().snapshot();
    for (id, secret) in fixture_secrets() {
        let held = reg.composed_transport(&id).unwrap().client_secret.unwrap_or_else(|| panic!("{id}: withheld"));
        assert!(held.matches(&secret), "{id}");
        assert!(!held.matches("not-the-secret"), "{id}");
        assert_eq!(format!("{held:?}"), "***");
        assert_eq!(format!("{held}"), "***");
        let json = serde_json::to_string(&reg.composed_transport(&id).unwrap()).unwrap();
        assert!(!json.contains(&secret) && !json.contains("clientSecret"), "{id}: secret serialised");
    }
}

/// A bundled plugin declaring every slice 005 section with public client data only.
const FULL: &str = r#"schema = 2
id = "acme"
category = "apikey"

[auth]
kind = "apikey"

[endpoints.text]
url = "https://api.acme.example/v1/responses"
wire = "openai-responses"

[signin]
flow = "pkce"
client_id = "b1a00492-073a-47ea-816f-4c329264a828"
authorize_url = "https://auth.acme.example/oauth2/authorize"
token_url = "https://auth.acme.example/oauth2/token"
redirect = [{ uri = "http://127.0.0.1:56121/callback", kind = "loopback" }]
params = { audience = "acme-api" }
refresh_lead = "5m"

[signin.profile]
url = "https://api.acme.example/v1/user"
headers = { x-acme-client = "cli" }
email = "email"

[identity.headers]
User-Agent = "acme-cli/1.0"
x-acme-session = "{session.id}"

[quota]
accounts = "signin"
request = { url = "https://api.acme.example/v1/usage", headers = { x-acme-beta = "usage-1" } }

[[quota.window]]
path = "five_hour"
name = "5-hour"
unit = "percent"
used = "utilization"

[models_live]
url = "https://api.acme.example/v1/models"
headers = { x-acme-mode = "headless" }
list = "data"
id = "id"

[[models]]
id = "m1"
"#;

fn gate_errors(src: &str) -> Vec<String> {
    use nullrouter_registry::validate::validate_with;
    use nullrouter_registry::{PluginSource, bundled_gate_ctx};
    let ctx = bundled_gate_ctx(true, false).unwrap();
    validate_with(src, PluginSource::Bundled, "acme.toml", &ctx)
        .err()
        .unwrap_or_default()
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// FR-031–FR-033, T087: the slice 005 sections have no field that can carry a secret.
/// Each declaration refuses credential-named fields at every level, as unknown fields.
#[test]
fn no_sign_in_identity_quota_or_live_model_field_holds_a_secret() {
    use nullrouter_registry::schema::{IdentityDecl, ModelsLiveDecl, QuotaDecl, SignInDecl};

    assert!(gate_errors(FULL).is_empty(), "{:#?}", gate_errors(FULL));
    let doc: toml::Table = toml::from_str(FULL).unwrap();
    let section = |k: &str| doc[k].as_table().unwrap().clone();
    let fields =
        ["client_secret", "access_token", "refresh_token", "id_token", "api_key", "password", "secret", "token"];

    // `(declaration, its section, the nested tables inside it)`.
    type Parse = fn(toml::Table) -> Result<(), String>;
    fn parse<T: serde::de::DeserializeOwned>(t: toml::Table) -> Result<(), String> {
        t.try_into::<T>().map(drop).map_err(|e| e.to_string())
    }
    let decls: [(&str, Parse, &[&str]); 4] = [
        ("signin", parse::<SignInDecl>, &["profile"]),
        ("identity", parse::<IdentityDecl>, &[]),
        ("quota", parse::<QuotaDecl>, &["request"]),
        ("models_live", parse::<ModelsLiveDecl>, &[]),
    ];
    for (name, parse, nested) in decls {
        let base = section(name);
        parse(base.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
        for field in fields {
            let mut t = base.clone();
            t.insert(field.into(), "opaque-value".into());
            let e = parse(t).expect_err(&format!("[{name}] took `{field}`"));
            assert!(e.contains(&format!("unknown field `{field}`")), "[{name}] {field}: {e}");
            for inner in nested {
                let mut t = base.clone();
                t[*inner].as_table_mut().unwrap().insert(field.into(), "opaque-value".into());
                let e = parse(t).expect_err(&format!("[{name}.{inner}] took `{field}`"));
                assert!(e.contains(&format!("unknown field `{field}`")), "[{name}.{inner}] {field}: {e}");
            }
        }
    }
}

/// FR-031, contract "Gate errors": a value that looks like a secret is refused wherever an
/// open string sits (client id, parameters, static and identity headers), and so are
/// credentials in a URL. Floor header names can't be set either.
#[test]
fn secret_like_values_are_refused() {
    const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.c2lnbmF0dXJlLXNpZ25hdHVyZQ";
    let cases: &[(&str, &str, &str)] = &[
        (
            "client_id = \"b1a00492-073a-47ea-816f-4c329264a828\"",
            "client_id = \"sk-ant-oat01-0123456789abcdefABCDEF\"",
            "signin.client_id: looks like a secret; plugins can't hold secrets",
        ),
        ("audience = \"acme-api\"", &format!("audience = \"{JWT}\""), "signin.params.audience: looks like a secret"),
        (
            "x-acme-client = \"cli\"",
            "x-acme-client = \"Bearer 0123456789abcdefABCDEF\"",
            "signin.profile.headers.x-acme-client: looks like a secret",
        ),
        (
            "User-Agent = \"acme-cli/1.0\"",
            "User-Agent = \"xai-0123456789abcdefABCDEFghij\"",
            "identity.headers.User-Agent: looks like a secret",
        ),
        (
            "x-acme-beta = \"usage-1\"",
            "x-acme-beta = \"ghp_0123456789abcdefABCDEF\"",
            "quota.request.headers.x-acme-beta: looks like a secret",
        ),
        (
            "x-acme-mode = \"headless\"",
            "x-acme-mode = \"aB3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dE3fG5\"",
            "models_live.headers.x-acme-mode: looks like a secret",
        ),
        ("x-acme-mode = \"headless\"", "Authorization = \"x\"", "models_live.headers.Authorization"),
        ("User-Agent = \"acme-cli/1.0\"", "x-api-key = \"{session.id}\"", "identity.headers.x-api-key"),
        (
            "https://auth.acme.example/oauth2/token",
            "https://client:hunter2@auth.acme.example/oauth2/token",
            "signin.token_url",
        ),
        (
            "https://api.acme.example/v1/usage",
            "https://api.acme.example/v1/usage?access_token=abc",
            "quota.request.url",
        ),
    ];
    for (from, to, want) in cases {
        assert!(FULL.contains(from), "{from}");
        let got = gate_errors(&FULL.replacen(from, to, 1));
        assert!(got.iter().any(|e| e.contains(want)), "{to}: want {want:?} in {got:#?}");
    }
}

/// T087: no plugin-visible type holds a secret. Everything a plugin file deserialises into
/// lives in `src/schema/`; none of it names `SecretString` or the credentials module, or
/// has a text field named for a credential. The core's secrets (`credentials`, the engine's
/// token store) stay out of reach of plugin data.
#[test]
fn no_plugin_visible_type_holds_a_secret() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/schema");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    assert!(files.len() >= 15, "{files:?}");
    let fields = ["client_secret", "access_token", "refresh_token", "id_token", "api_key", "password", "secret"];
    for path in files {
        let src = std::fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy();
        // Tests may spell the refused names; only the declarations count.
        let decls = src.split("#[cfg(test)]").next().unwrap();
        assert!(!decls.contains("SecretString"), "{name} holds a SecretString");
        assert!(!decls.contains("credentials::"), "{name} reaches the credentials table");
        for line in decls.lines().map(str::trim) {
            let Some((field, ty)) = line.strip_prefix("pub ").and_then(|l| l.split_once(':')) else {
                continue;
            };
            // A credential-named field may say where a credential goes (`api_key:
            // Option<AuthPlacement>`), never hold one as text.
            let text = matches!(ty.trim().trim_end_matches(','), "String" | "Option<String>" | "Vec<String>");
            assert!(!(text && fields.contains(&field.trim())), "{name}: field `{}`: {line}", field.trim());
        }
    }
}
