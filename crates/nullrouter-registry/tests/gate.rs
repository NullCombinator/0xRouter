//! US4 s1–s4, SC-003: the validation gate over `tests/gate/`.

use std::fs;
use std::path::{Path, PathBuf};

use nullrouter_registry::validate::{
    GateCtx, ValidationError, check_route_collisions, validate, validate_style, validate_with,
};
use nullrouter_registry::{
    CapabilityKind, OperatorHome, PluginSource, RegistryHandle, bundled_gate_ctx, bundled_sources,
    bundled_style_sources, community, validate_user_plugin,
};

/// Rules whose error must list the allowed values.
const ENUM_RULES: &[&str] = &[
    "bad-category",
    "unknown-capability",
    "unknown-executor-param",
    "unknown-format",
    "unknown-hook",
    "unknown-key",
    "unknown-oauth-param",
    "unknown-quirk",
];

fn corpus(dir: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate").join(dir);
    let mut files: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_file()).collect();
    files.sort();
    files
}

fn load(path: &Path) -> nullrouter_registry::ProviderEntity {
    let src = fs::read_to_string(path).unwrap();
    validate_user_plugin(&src, path).unwrap_or_else(|e| panic!("{}: {e:?}", path.display()))
}

fn valid(name: &str) -> nullrouter_registry::ProviderEntity {
    load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/valid").join(name))
}

#[test]
fn valid_corpus_is_accepted() {
    let files = corpus("valid");
    assert_eq!(files.len(), 4);
    for f in &files {
        load(f);
    }
    assert!(valid("minimal.toml").transport.is_none());
}

#[test]
fn invalid_corpus_is_rejected_with_one_positioned_error() {
    let files = corpus("invalid");
    assert_eq!(files.len(), 23);
    for path in files {
        let src = fs::read_to_string(&path).unwrap();
        let rule = path.file_stem().unwrap().to_str().unwrap();
        let expect = src.lines().next().and_then(|l| l.strip_prefix("# expect: ")).expect("# expect: line");
        let errors = validate_user_plugin(&src, &path).expect_err(rule);
        assert_eq!(errors.len(), 1, "{rule}: {errors:?}");
        let e = &errors[0];
        let text = e.to_string();
        let file = path.display().to_string();
        assert!(e.line > 0 && e.col > 0, "{rule}: unpositioned: {text}");
        assert!(text.starts_with(&format!("{file}:{}:{}", e.line, e.col)), "{rule}: {text}");
        assert!(text.contains(expect), "{rule}: {text:?} lacks {expect:?}");
        if !e.path.is_root() {
            assert!(text.contains(&format!(" {}: ", e.path)), "{rule}: {text}");
        }
        if ENUM_RULES.contains(&rule) {
            assert!(text.contains("allowed: ") || text.contains("expected one of "), "{rule}: {text}");
        }
    }
}

#[test]
fn sections_are_capabilities() {
    let p = valid("llm-embedding.toml");
    assert!(p.capabilities.contains_key(&CapabilityKind::Llm));
    assert!(p.capabilities.contains_key(&CapabilityKind::Embedding));
    assert_eq!(p.capabilities.len(), 2);

    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join("plugins")).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/valid/llm-embedding.toml");
    fs::copy(src, home.path().join("plugins/two-kinds.toml")).unwrap();
    let reg = RegistryHandle::open(OperatorHome::new(home.path())).unwrap().snapshot();
    assert!(reg.capability("two-kinds", CapabilityKind::Llm).unwrap().is_some());
    assert!(reg.capability("two-kinds", CapabilityKind::Embedding).unwrap().is_some());
    assert!(reg.capability("two-kinds", CapabilityKind::Tts).unwrap().is_none());
}

#[test]
fn bare_models_get_ids_and_derived_names() {
    let p = valid("bare-models.toml");
    let models = p.models.as_deref().unwrap();
    assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["m-a", "m-b"]);

    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join("plugins")).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/valid/bare-models.toml");
    fs::copy(src, home.path().join("plugins/bare.toml")).unwrap();
    // A catalog-only plugin: slice 002's set, with the fit check off.
    let reg = RegistryHandle::open_parity(OperatorHome::new(home.path())).unwrap().snapshot();
    let info = reg.model("bare", "m-a").unwrap();
    assert!(info.declared);
    assert_eq!(info.name, nullrouter_registry::derive_model_name("m-a"));
}

/// FR-005: absent `models` (catalog unknown) differs from `models = []` (offers none).
#[test]
fn catalog_unknown_is_not_catalog_empty() {
    assert!(valid("minimal.toml").models.is_none());
    let (_, zed) = community::COMMUNITY.iter().find(|(f, _)| *f == "zed.toml").unwrap();
    assert!(zed.lines().any(|l| l.trim() == "models = []"), "zed.toml no longer declares an empty catalog");
    let zed = validate(zed, PluginSource::Bundled, "zed.toml").unwrap();
    assert_eq!(zed.models.as_deref(), Some(&[][..]));
}

#[test]
fn every_bundled_and_community_plugin_passes_the_gate() {
    assert_eq!(bundled_sources().len(), 7);
    assert_eq!(community::COMMUNITY.len(), 114);
    // With the bundled styles loaded, in the bundled set's strict mode. (A community plugin's
    // `credential_fallback` may name another community plugin: that is the fit check's.)
    // The self-hosted community plugins point at localhost, so they need private endpoints.
    for (file, src) in bundled_sources() {
        validate_with(src, PluginSource::Bundled, file, &ctx(true, false)).unwrap_or_else(|e| panic!("{file}: {e:?}"));
    }
    for (file, src) in community::COMMUNITY {
        validate_with(src, PluginSource::Bundled, file, &ctx(true, true)).unwrap_or_else(|e| panic!("{file}: {e:?}"));
    }
}

/// The plugins kept by hand rather than generated from 9router.
const HAND_MAINTAINED: [&str; 5] =
    ["anthropic.toml", "elevenlabs.toml", "opencode-go.toml", "opencode-zen.toml", "openrouter.toml"];

/// The `# expect:` first line of a corpus file.
fn expect(src: &str) -> &str {
    src.lines().next().and_then(|l| l.strip_prefix("# expect: ")).expect("# expect: line")
}

/// Exactly one positioned error, naming `file` and containing the expected text.
fn one_error(rule: &str, file: &str, want: &str, errors: &[ValidationError]) {
    assert_eq!(errors.len(), 1, "{rule}: {errors:#?}");
    let text = errors[0].to_string();
    assert!(errors[0].line > 0 && errors[0].col > 0, "{rule}: unpositioned: {text}");
    assert!(text.starts_with(file), "{rule}: {text}");
    assert!(text.contains(want), "{rule}: {text:?} lacks {want:?}");
}

fn ctx(strict: bool, allow_private: bool) -> GateCtx {
    bundled_gate_ctx(strict, allow_private).expect("bundled styles load")
}

/// US7, T117: each style case fails with its one expected error; the two collision files
/// pass alone, and together get one collision error each.
#[test]
fn style_corpus_is_rejected_with_one_positioned_error() {
    let files = corpus("invalid/styles");
    assert_eq!(files.len(), 13);
    let mut pair = Vec::new();
    for path in &files {
        let src = fs::read_to_string(path).unwrap();
        let rule = path.file_stem().unwrap().to_str().unwrap();
        let file = path.display().to_string();
        if rule.starts_with("route-collision") {
            let style = validate_style(&src, &file).unwrap_or_else(|e| panic!("{rule}: {e:#?}"));
            pair.push((file, src, style));
            continue;
        }
        one_error(rule, &file, expect(&src), &validate_style(&src, &file).expect_err(rule));
    }
    let pair: Vec<_> = pair.iter().map(|(f, s, st)| (f.as_str(), s.as_str(), st)).collect();
    let errors = check_route_collisions(&pair);
    // One error per file, each at that file's route.
    assert_eq!(errors.len(), 2, "{errors:#?}");
    for ((file, src, _), e) in pair.iter().zip(&errors) {
        one_error("route-collision", file, expect(src), std::slice::from_ref(e));
    }
}

/// US7, T118 and slice 005 T013: each schema-2 case fails with its one expected error,
/// gated as a user plugin is against the bundled styles. A case with a golden `.expected`
/// file must match it exactly; `NR_BLESS=1` rewrites the goldens.
#[test]
fn provider_corpus_is_rejected_with_one_positioned_error() {
    let files: Vec<_> =
        corpus("invalid/providers").into_iter().filter(|p| p.extension().is_some_and(|e| e == "toml")).collect();
    assert_eq!(files.len(), 25);
    let mut goldens = 0;
    for path in &files {
        let src = fs::read_to_string(path).unwrap();
        let rule = path.file_stem().unwrap().to_str().unwrap();
        let file = format!("providers/{}", path.file_name().unwrap().to_str().unwrap());
        let errors =
            validate_with(&src, PluginSource::User(path.clone()), &file, &ctx(false, false)).err().unwrap_or_default();
        one_error(rule, &file, expect(&src), &errors);
        let golden = path.with_extension("expected");
        if std::env::var_os("NR_BLESS").is_some() && golden.exists() {
            fs::write(&golden, format!("{}\n", errors[0])).unwrap();
        }
        if let Ok(want) = fs::read_to_string(&golden) {
            goldens += 1;
            assert_eq!(errors[0].to_string(), want.trim_end_matches('\n'), "{rule}");
        }
    }
    assert_eq!(goldens, 6, "slice 005 cases carry goldens");
}

/// Slice 005: a plugin declaring every new section passes the gate in strict mode as a
/// bundled plugin, fits as one, and is refused whole as a user (community) plugin.
#[test]
fn signin_sections_pass_the_gate_and_fit_only_when_bundled() {
    use nullrouter_registry::fit::{self, FitVerdict};
    use nullrouter_registry::schema::{
        ForcedParam, HeaderValue, LiveModelType, ModelType, Placeholder, QuotaDecoder, SignInFlow, SignInParam,
        SignInParamValue,
    };

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/signin/full.toml");
    let src = fs::read_to_string(&path).unwrap();
    let strict = ctx(true, false);
    let g = validate_with(&src, PluginSource::Bundled, "full.toml", &strict).unwrap_or_else(|e| panic!("{e:#?}"));
    assert!(g.diagnostics.is_empty(), "{:?}", g.diagnostics);
    let p = &g.entity;
    assert_eq!(fit::check(p, &src, "full.toml", &strict), FitVerdict::Fits);

    let s = p.signin.as_ref().unwrap();
    assert_eq!(s.flow, SignInFlow::Pkce);
    assert_eq!(s.params[&SignInParam::Nonce], SignInParamValue::RandomHex16);
    assert_eq!(s.redirect.len(), 2);
    let id = p.identity.as_ref().unwrap();
    assert_eq!(id.headers["x-grok-agent-id"], HeaderValue::Core(Placeholder::InstallId));
    assert_eq!(id.placeholders().count(), 8);
    let q = p.quota.as_ref().unwrap();
    assert_eq!(q.primary.windows.len(), 2);
    assert_eq!(q.fallback.as_ref().unwrap().decoder, QuotaDecoder::GrpcWebRatio);
    assert_eq!(p.models_live.as_ref().unwrap().model_type, LiveModelType::Fixed(ModelType::Text));
    let text = &p.endpoints[&ModelType::Text].0[0];
    assert_eq!(
        text.force.iter().map(|(k, _)| k).collect::<Vec<_>>(),
        [ForcedParam::Store, ForcedParam::ReasoningSummary, ForcedParam::Include]
    );
    let high = &p.models.as_ref().unwrap()[1];
    assert_eq!(high.force.0[&ForcedParam::ReasoningEffort].as_str(), Some("high"));
    assert!(p.models.as_ref().unwrap()[0].force.is_empty());

    // Tokens are bound to the endpoint, sign-in, quota (with fallback) and live-model hosts;
    // the hosted code page and the loopback redirect never receive one.
    let hosts: Vec<_> = p.token_hosts().into_iter().collect();
    assert_eq!(hosts, ["auth.grokish.example", "cli-chat-proxy.grokish.example", "grokish.example"]);

    let user = validate_with(&src, PluginSource::User(path.clone()), "full.toml", &strict).unwrap().entity;
    let FitVerdict::Unsupported { parts } = fit::check(&user, &src, "full.toml", &strict) else { panic!("fits") };
    let reasons: Vec<_> = parts.iter().map(|p| format!("{}: {}", p.path, p.reason)).collect();
    assert_eq!(
        reasons,
        [
            "signin: account sign-in is not supported",
            "identity: account sign-in is not supported",
            "models_live: account sign-in is not supported",
            "quota: quota is not supported",
        ]
    );
}

/// Slice 005 gate rules beyond the corpus: flow fields, ranges, redirects, host set, quota
/// decoders and window names.
#[test]
fn signin_gate_rules() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/signin/full.toml");
    let src = fs::read_to_string(path).unwrap();
    let strict = ctx(true, false);
    let fails = |from: &str, to: &str, want: &str| {
        assert!(src.contains(from), "{from}");
        let changed = src.replacen(from, to, 1);
        let got: Vec<String> = validate_with(&changed, PluginSource::Bundled, "t.toml", &strict)
            .err()
            .unwrap_or_default()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(got.iter().any(|e| e.contains(want)), "{from} -> {to}: want {want:?} in {got:#?}");
    };
    fails("verifier_bytes = 96", "verifier_bytes = 97", "verifier_bytes must be 32-96");
    fails("verifier_bytes = 96", "verifier_bytes = 16", "verifier_bytes must be 32-96");
    fails(
        "discovery_url = \"https://auth.grokish.example/.well-known/openid-configuration\"\nauthorize_url = \"https://auth.grokish.example/oauth2/authorize\"\n",
        "",
        "needs `authorize_url` or `discovery_url`",
    );
    fails(
        "token_url = \"https://auth",
        "device_url = \"https://auth.grokish.example/d\"\ntoken_url = \"https://auth",
        "`device_url` is not used by the pkce flow",
    );
    fails("http://127.0.0.1:56121/callback", "http://10.0.0.1:56121/callback", "a loopback redirect is");
    fails("https://console.grokish.example/oauth/code/callback", "http://127.0.0.1/cb", "loopback, private");
    fails(
        "https://auth.grokish.example/oauth2/token",
        "http://169.254.169.254/token",
        "signin.token_url: host 169.254.169.254",
    );
    fails(
        "https://auth.grokish.example/oauth2/token",
        "https://auth.grokish.example/{model}/token",
        "placeholders are not allowed",
    );
    fails("plan = \"generic\"", "plan = \"sk-proj-0123456789abcdefABCDEF\"", "signin.params.plan: looks like a secret");
    fails("plan = \"generic\"", "scope = \"x\"", "allowed: plan, referrer, code, audience, prompt, nonce");
    fails("referrer = \"cli-proxy-api\"", "referrer = \"{random.uuid}\"", "unknown placeholder {random.uuid}");
    fails("refresh_lead = \"5m\"", "refresh_lead = \"0s\"", "signin.refresh_lead: must be more than 0");
    fails("refresh_lead = \"5m\"", "refresh_lead = \"soon\"", "not a duration");
    fails("scheme = \"bearer\" }", "scheme = \"<user_id> <access_token>\" }", "takes `bearer` or `raw`");
    fails("status = [400, 403]", "status = [99]", "statuses must be 100-599");
    fails("x-grok-client-identifier = \"grok-shell\"", "Cookie = \"a=b\"", "\"Cookie\" is in the security floor");
    fails(
        "x-grok-client-identifier = \"grok-shell\"",
        "x-grok-client-identifier = \"id-{session.id}\"",
        "exactly one placeholder",
    );
    fails(
        "x-grok-client-identifier = \"grok-shell\"",
        "x-grok-client-identifier = \"ghp_0123456789abcdefABCDEF\"",
        "identity.headers.x-grok-client-identifier: looks like a secret",
    );
    fails(
        "https://grokish.example/grok_api_v2",
        "https://other.example/grok_api_v2",
        "quota.fallback.request.url: host other.example is not one",
    );
    fails(
        "url = \"https://cli-chat-proxy.grokish.example/v1/models\"",
        "url = \"https://models.example/v1/models\"",
        "models_live.url: host models.example",
    );
    fails(
        "url = \"https://cli-chat-proxy.grokish.example/v1/user\"",
        "url = \"https://me.example/v1/user\"",
        "signin.profile.url: host me.example",
    );
    fails(
        "decoder = \"grpc_web_ratio\"\nname = \"credits\"\n",
        "decoder = \"grpc_web_ratio\"\n",
        "needs `name` and `unit`",
    );
    fails("decoder = \"grpc_web_ratio\"", "decoder = \"json\"", "needs at least one [[window]] rule");
    fails("name = \"weekly {1}\"", "name = \"weekly {2}\"", "{2}: the path binds 1 `*`");
    fails("used = \"utilization\"\n", "", "a window needs `used`, `limit` or `remaining`");
    fails("unit = \"credits\"", "unit = \"dollars\"", "allowed: percent, credits, requests, tokens");
    fails(
        "x-grok-client-mode = \"headless\" } }",
        "Authorization = \"x\" } }",
        "\"Authorization\" is in the security floor; the core sets it",
    );
    fails("store = false", "store = \"no\"", "store must be a boolean");
    fails("\"reasoning.effort\" = \"high\"", "\"reasoning.effort\" = \"high\", tools = []", "tools can't be forced");
    fails("[signin]\nflow", "[signin.x]\n[signin]\nflow", "unknown field `x`");
    // Without [signin], the sections that need a sign-in account are refused.
    let no_signin =
        src.split("\n[signin]").next().unwrap().to_owned() + "\n" + &src[src.find("[identity.headers]").unwrap()..];
    let got: Vec<String> = validate_with(&no_signin, PluginSource::Bundled, "t.toml", &strict)
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect();
    for want in
        ["declare [signin]", "`signin` needs a [signin] section", "[models_live] is read with a sign-in account"]
    {
        assert!(got.iter().any(|e| e.contains(want)), "{want}: {got:#?}");
    }
    // Schema 1 has none of them.
    let s1 = "schema = 1\nid = \"p\"\ncategory = \"apikey\"\n[identity.headers]\nx = \"y\"\n[[models]]\nid = \"m\"\nforce = { store = false }\n";
    let got: Vec<String> = validate_with(s1, PluginSource::Bundled, "t.toml", &strict)
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(got.iter().any(|e| e.contains("schema 1 has no `identity`")), "{got:#?}");
    assert!(got.iter().any(|e| e.contains("models[0].force: schema 1 has no `force`")), "{got:#?}");
}

#[test]
fn a_private_endpoint_passes_when_the_operator_allows_it() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/invalid/providers/url-localhost.toml");
    let src = fs::read_to_string(&path).unwrap();
    let g = validate_with(&src, PluginSource::User(path.clone()), "url-localhost.toml", &ctx(false, true)).unwrap();
    assert!(g.diagnostics.is_empty(), "{:?}", g.diagnostics);
}

/// A floor name in a forwarding list is stripped with a diagnostic, and is an error in
/// strict mode (the bundled plugins' mode).
#[test]
fn a_floor_name_is_stripped_or_refused_in_strict_mode() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/strict/forwarding-authorization.toml");
    let src = fs::read_to_string(&path).unwrap();
    let g = validate_with(&src, PluginSource::User(path.clone()), "f.toml", &ctx(false, false)).unwrap();
    let up = &g.entity.forwarding.as_ref().unwrap().to_upstream.headers;
    assert!(up.iter().all(|h| h.name != "authorization"), "{up:?}");
    assert_eq!(g.diagnostics.len(), 1, "{:?}", g.diagnostics);
    assert!(g.diagnostics[0].to_string().contains("entry stripped"), "{:?}", g.diagnostics);
    let errors = validate_with(&src, PluginSource::User(path.clone()), "f.toml", &ctx(true, false)).unwrap_err();
    one_error("forwarding-authorization", "f.toml", expect(&src), &errors);
}

/// US7-4: the shipped styles and the hand-maintained plugins pass in strict mode.
#[test]
fn shipped_styles_and_hand_maintained_plugins_pass_strict() {
    let styles = bundled_style_sources();
    assert_eq!(styles.len(), 4);
    for (file, src) in styles {
        validate_style(src, file).unwrap_or_else(|e| panic!("{file}: {e:#?}"));
    }
    let strict = ctx(true, false);
    for name in HAND_MAINTAINED {
        let (_, src) = bundled_sources().iter().find(|(f, _)| *f == name).unwrap();
        let g = validate_with(src, PluginSource::Bundled, name, &strict).unwrap_or_else(|e| panic!("{name}: {e:#?}"));
        assert!(g.diagnostics.is_empty(), "{name}: {:?}", g.diagnostics);
    }
}
