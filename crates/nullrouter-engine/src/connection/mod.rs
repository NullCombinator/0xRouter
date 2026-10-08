//! Effective connection settings, the client cache and proxies (spec 013).

pub mod clients;
pub mod proxy;

use std::time::Duration;

use nullrouter_registry::Registry;
use nullrouter_registry::schema::{Endpoint, ProviderEntity, ProviderSettings};

use crate::records::{Source, SourceBy, SourceLevel, TimeoutKind};
use crate::upstream::{DEFAULT_STALL_MS, DEFAULT_TIMEOUT_MS, parse_ms};

/// One timeout and the level that set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeout {
    pub ms: u64,
    pub source: Source,
}

impl Timeout {
    pub fn duration(self) -> Duration {
        Duration::from_millis(self.ms)
    }
}

/// What one attempt runs with, resolved from the snapshot the request holds (FR-031).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    pub connect: Timeout,
    /// From the attempt's start, connect included (research R12).
    pub headers: Timeout,
    /// `None`: off, which is the built-in default.
    pub first_token: Option<Timeout>,
    pub stall: Timeout,
}

impl Effective {
    pub fn hit(&self, which: TimeoutKind) -> Option<Timeout> {
        match which {
            TimeoutKind::Connect => Some(self.connect),
            TimeoutKind::Headers => Some(self.headers),
            TimeoutKind::FirstToken => self.first_token,
            TimeoutKind::Stall => Some(self.stall),
        }
    }
}

fn at(by: SourceBy, level: SourceLevel) -> Source {
    Source { by, level }
}

/// `candidate`'s settings for `provider`, with the operator's `config.toml` read from the
/// snapshot's registry.
pub fn effective(
    registry: &Registry,
    provider: &ProviderEntity,
    requested: &str,
    upstream_id: &str,
    endpoint: &Endpoint,
) -> Effective {
    let settings = registry.settings(&provider.id);
    resolve(&settings, provider, requested, upstream_id, endpoint, &|name| std::env::var(name).ok())
}

/// Operator model → operator provider → plugin model → plugin endpoint → built-in (the
/// environment, then the default). `env` reads a variable; tests pass their own.
pub fn resolve(
    settings: &ProviderSettings,
    provider: &ProviderEntity,
    requested: &str,
    upstream_id: &str,
    endpoint: &Endpoint,
    env: &dyn Fn(&str) -> Option<String>,
) -> Effective {
    let op_model = [requested, upstream_id].into_iter().find_map(|id| settings.model.get(id)).map(|m| &m.connection);
    let plugin_model = provider
        .models
        .iter()
        .flatten()
        .find(|m| m.id == requested || m.id == upstream_id)
        .and_then(|m| m.timeouts);
    let op = &settings.connection;
    let built_in = |var: &str, default: u64| match env(var).map(|raw| parse_ms(&raw, 0)).filter(|n| *n > 0) {
        Some(ms) => Timeout { ms, source: at(SourceBy::BuiltIn, SourceLevel::Env) },
        None => Timeout { ms: default, source: at(SourceBy::BuiltIn, SourceLevel::Default) },
    };
    let pick = |op_model: Option<u64>, op_provider: Option<u64>, plugin_model: Option<u64>, plugin_endpoint: Option<u64>| {
        [
            (op_model, at(SourceBy::Operator, SourceLevel::Model)),
            (op_provider, at(SourceBy::Operator, SourceLevel::Provider)),
            (plugin_model, at(SourceBy::Plugin, SourceLevel::Model)),
            (plugin_endpoint, at(SourceBy::Plugin, SourceLevel::Endpoint)),
        ]
        .into_iter()
        .find_map(|(ms, source)| ms.map(|ms| (ms, source)))
    };
    let timeout = |found: Option<(u64, Source)>, fallback: Timeout| match found {
        Some((ms, source)) => Timeout { ms, source },
        None => fallback,
    };
    let connect = timeout(
        pick(
            op_model.and_then(|m| m.connect_timeout_ms),
            op.connect_timeout_ms,
            plugin_model.and_then(|m| m.connect_ms),
            endpoint.connect_timeout_ms,
        ),
        built_in("FETCH_CONNECT_TIMEOUT_MS", DEFAULT_TIMEOUT_MS),
    );
    let headers = timeout(
        pick(
            op_model.and_then(|m| m.header_timeout_ms),
            op.header_timeout_ms,
            plugin_model.and_then(|m| m.headers_ms),
            endpoint.timeout_ms,
        ),
        built_in("FETCH_CONNECT_TIMEOUT_MS", DEFAULT_TIMEOUT_MS),
    );
    let stall = timeout(
        pick(
            op_model.and_then(|m| m.stall_timeout_ms),
            op.stall_timeout_ms,
            plugin_model.and_then(|m| m.stall_ms),
            endpoint.stall_timeout_ms,
        ),
        built_in("STREAM_STALL_TIMEOUT_MS", DEFAULT_STALL_MS),
    );
    // Off is a value: 0 at a level ends the search and leaves no deadline.
    let first_token = pick(
        op_model.and_then(|m| m.first_token_timeout_ms),
        op.first_token_timeout_ms,
        plugin_model.and_then(|m| m.first_token_ms),
        endpoint.first_token_timeout_ms,
    )
    .filter(|(ms, _)| *ms > 0)
    .map(|(ms, source)| Timeout { ms, source });
    Effective { connect, headers, first_token, stall }
}

/// The level a proxy assignment came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyLevel {
    Account,
    Provider,
    All,
}

/// The proxy one attempt goes through. `name: None` with a level is an explicit `none`; with no
/// level, nothing assigned one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChosenProxy {
    pub name: Option<String>,
    pub level: Option<ProxyLevel>,
}

/// Account, then provider, then all providers; the first that says anything wins, and `none`
/// is an answer (it stops the search with no proxy).
pub fn proxy_for(registry: &Registry, provider: &str, account: Option<&crate::accounts::Account>) -> ChosenProxy {
    let pick = |value: Option<&String>, level| {
        value.map(|v| ChosenProxy { name: (v != proxy::NONE).then(|| v.clone()), level: Some(level) })
    };
    pick(account.and_then(|a| a.proxy.as_ref()), ProxyLevel::Account)
        .or_else(|| pick(registry.settings(provider).connection.proxy.as_ref(), ProxyLevel::Provider))
        .or_else(|| pick(registry.runtime().connection_proxy.as_ref(), ProxyLevel::All))
        .unwrap_or(ChosenProxy { name: None, level: None })
}

fn timeout_json(t: Option<Timeout>) -> serde_json::Value {
    match t {
        Some(t) => serde_json::json!({"ms": t.ms, "source": t.source}),
        None => serde_json::json!({"ms": null, "source": null}),
    }
}

fn timeouts_json(e: &Effective) -> serde_json::Value {
    serde_json::json!({
        "connect": timeout_json(Some(e.connect)),
        "headers": timeout_json(Some(e.headers)),
        "first_token": timeout_json(e.first_token),
        "stall": timeout_json(Some(e.stall)),
    })
}

/// The `connection.view` answer (contracts/operator-socket.md): per provider, each timeout with
/// where it came from, and the models whose timeouts differ from the provider's. `only` names
/// one provider; an unknown one is an error listing the known ones.
pub fn view(registry: &Registry, only: Option<&str>) -> Result<serde_json::Value, String> {
    use nullrouter_registry::schema::ModelType;

    let known: Vec<&ProviderEntity> = registry.providers().collect();
    if let Some(name) = only
        && !known.iter().any(|p| p.id == name)
    {
        let ids: Vec<&str> = known.iter().map(|p| p.id.as_str()).collect();
        return Err(format!("no provider {name:?}; known providers: {}", ids.join(", ")));
    }
    let mut out = Vec::new();
    for p in known.iter().filter(|p| only.is_none_or(|n| p.id == n)) {
        // The timeouts of a provider are those of its text endpoint, else its first endpoint.
        let Some(endpoint) = p
            .endpoints
            .get(&ModelType::Text)
            .or_else(|| p.endpoints.values().next())
            .and_then(|e| e.0.first())
        else {
            continue;
        };
        let settings = registry.settings(&p.id);
        let base = effective(registry, p, "", "", endpoint);
        let mut ids: Vec<&str> = p.models.iter().flatten().map(|m| m.id.as_str()).collect();
        ids.extend(settings.model.keys().map(String::as_str));
        ids.sort_unstable();
        ids.dedup();
        let models: Vec<serde_json::Value> = ids
            .into_iter()
            .filter_map(|id| {
                let e = effective(registry, p, id, id, endpoint);
                (e != base).then(|| serde_json::json!({"id": id, "timeouts": timeouts_json(&e)}))
            })
            .collect();
        out.push(serde_json::json!({"id": p.id, "timeouts": timeouts_json(&base), "models": models}));
    }
    Ok(serde_json::json!({"ok": true, "providers": out}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nullrouter_registry::schema::{ConnectionSettings, ModelConnection, ModelSettings, ModelType};

    const PLUGIN: &str = r#"
schema = 2
id = "acme"
category = "apikey"
[auth]
header = "x-api-key"
scheme = "raw"
[[endpoints.text]]
url = "https://api.acme.example/v1/messages"
wire = "anthropic-messages"
[[models]]
id = "slow"
kind = "llm"
"#;

    fn provider() -> ProviderEntity {
        nullrouter_registry::validate_user_plugin(PLUGIN, std::path::Path::new("acme.toml"))
            .unwrap_or_else(|e| panic!("{e:#?}"))
    }

    fn endpoint(p: &ProviderEntity) -> Endpoint {
        p.endpoints[&ModelType::Text].0[0].clone()
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn operator_model(c: ModelConnection) -> ProviderSettings {
        ProviderSettings {
            model: [("slow".to_owned(), ModelSettings { connection: c })].into(),
            ..ProviderSettings::default()
        }
    }

    fn src(by: SourceBy, level: SourceLevel) -> Source {
        Source { by, level }
    }

    #[test]
    fn with_nothing_set_the_built_ins_apply_and_first_token_is_off() {
        let p = provider();
        let e = resolve(&ProviderSettings::default(), &p, "slow", "slow", &endpoint(&p), &no_env);
        let d = src(SourceBy::BuiltIn, SourceLevel::Default);
        assert_eq!(e.connect, Timeout { ms: 60_000, source: d });
        assert_eq!(e.headers, Timeout { ms: 60_000, source: d });
        assert_eq!(e.stall, Timeout { ms: 360_000, source: d });
        assert_eq!(e.first_token, None);
    }

    #[test]
    fn the_environment_is_the_built_in_level_for_connect_headers_and_stall() {
        let p = provider();
        let env = |n: &str| match n {
            "FETCH_CONNECT_TIMEOUT_MS" => Some("7000".to_owned()),
            "STREAM_STALL_TIMEOUT_MS" => Some("9000ms".to_owned()),
            _ => None,
        };
        let e = resolve(&ProviderSettings::default(), &p, "slow", "slow", &endpoint(&p), &env);
        let v = src(SourceBy::BuiltIn, SourceLevel::Env);
        assert_eq!((e.connect, e.headers), (Timeout { ms: 7000, source: v }, Timeout { ms: 7000, source: v }));
        assert_eq!(e.stall, Timeout { ms: 9000, source: v });
        // A value that isn't a positive number is ignored, as `env_ms` does.
        let bad = |_: &str| Some("0".to_owned());
        let e = resolve(&ProviderSettings::default(), &p, "slow", "slow", &endpoint(&p), &bad);
        assert_eq!(e.connect.source, src(SourceBy::BuiltIn, SourceLevel::Default));
    }

    /// Each step beats every step below it, for each of the four timeouts.
    #[test]
    fn every_precedence_step_wins_over_the_ones_below_it() {
        let mut p = provider();
        let mut ep = endpoint(&p);
        let none = ProviderSettings::default();
        let tm = |k: u64| nullrouter_registry::schema::ModelTimeouts {
            connect_ms: Some(k),
            headers_ms: Some(k),
            first_token_ms: Some(k),
            stall_ms: Some(k),
        };
        let all = |k: u64| (Some(k), Some(k), Some(k), Some(k));
        let get = |e: &Effective| [Some(e.connect), Some(e.headers), e.first_token, Some(e.stall)];

        ep.connect_timeout_ms = Some(1);
        ep.timeout_ms = Some(1);
        ep.first_token_timeout_ms = Some(1);
        ep.stall_timeout_ms = Some(1);
        let e = resolve(&none, &p, "slow", "slow", &ep, &no_env);
        for t in get(&e) {
            assert_eq!(t, Some(Timeout { ms: 1, source: src(SourceBy::Plugin, SourceLevel::Endpoint) }));
        }

        p.models.as_mut().unwrap()[0].timeouts = Some(tm(2));
        let e = resolve(&none, &p, "slow", "slow", &ep, &no_env);
        for t in get(&e) {
            assert_eq!(t, Some(Timeout { ms: 2, source: src(SourceBy::Plugin, SourceLevel::Model) }));
        }
        // A plugin model that sets only one leaves the rest to the endpoint.
        p.models.as_mut().unwrap()[0].timeouts =
            Some(nullrouter_registry::schema::ModelTimeouts { stall_ms: Some(2), ..Default::default() });
        let e = resolve(&none, &p, "slow", "slow", &ep, &no_env);
        assert_eq!(e.stall.source, src(SourceBy::Plugin, SourceLevel::Model));
        assert_eq!(e.connect.source, src(SourceBy::Plugin, SourceLevel::Endpoint));
        p.models.as_mut().unwrap()[0].timeouts = Some(tm(2));

        let (c, h, f, s) = all(3);
        let provider_level = ProviderSettings {
            connection: ConnectionSettings {
                connect_timeout_ms: c,
                header_timeout_ms: h,
                first_token_timeout_ms: f,
                stall_timeout_ms: s,
                ..Default::default()
            },
            ..ProviderSettings::default()
        };
        let e = resolve(&provider_level, &p, "slow", "slow", &ep, &no_env);
        for t in get(&e) {
            assert_eq!(t, Some(Timeout { ms: 3, source: src(SourceBy::Operator, SourceLevel::Provider) }));
        }

        let (c, h, f, s) = all(4);
        let mut model_level = operator_model(ModelConnection {
            connect_timeout_ms: c,
            header_timeout_ms: h,
            first_token_timeout_ms: f,
            stall_timeout_ms: s,
        });
        model_level.connection = provider_level.connection.clone();
        let e = resolve(&model_level, &p, "slow", "slow", &ep, &no_env);
        for t in get(&e) {
            assert_eq!(t, Some(Timeout { ms: 4, source: src(SourceBy::Operator, SourceLevel::Model) }));
        }
        // Another model of the provider doesn't see it.
        let e = resolve(&model_level, &p, "fast", "fast", &ep, &no_env);
        assert_eq!(e.stall.source, src(SourceBy::Operator, SourceLevel::Provider));
    }

    #[test]
    fn a_first_token_timeout_of_zero_turns_it_off_at_any_level() {
        let p = provider();
        let mut ep = endpoint(&p);
        ep.first_token_timeout_ms = Some(120_000);
        let off = ProviderSettings {
            connection: ConnectionSettings { first_token_timeout_ms: Some(0), ..Default::default() },
            ..ProviderSettings::default()
        };
        assert_eq!(resolve(&off, &p, "slow", "slow", &ep, &no_env).first_token, None);
        let e = resolve(&ProviderSettings::default(), &p, "slow", "slow", &ep, &no_env);
        assert_eq!(e.first_token.map(|t| t.ms), Some(120_000));
        // Off at the model beats a provider value.
        let mut s = operator_model(ModelConnection { first_token_timeout_ms: Some(0), ..Default::default() });
        s.connection.first_token_timeout_ms = Some(5000);
        assert_eq!(resolve(&s, &p, "slow", "slow", &ep, &no_env).first_token, None);
    }

    #[test]
    fn the_operator_model_key_matches_the_requested_or_the_upstream_id() {
        let p = provider();
        let s = operator_model(ModelConnection { stall_timeout_ms: Some(11), ..Default::default() });
        let by_up = resolve(&s, &p, "alias", "slow", &endpoint(&p), &no_env);
        assert_eq!(by_up.stall.ms, 11);
    }

    fn registry(config: &str) -> (tempfile::TempDir, std::sync::Arc<Registry>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        let handle = nullrouter_registry::RegistryHandle::open_parity(nullrouter_registry::OperatorHome::new(dir.path()))
            .unwrap_or_else(|e| panic!("{e}"));
        let snapshot = handle.snapshot();
        (dir, snapshot)
    }

    fn accounts(text: &str) -> crate::accounts::Accounts {
        crate::accounts::Accounts::parse(text, std::path::Path::new("accounts.toml"), |_| None).unwrap()
    }

    fn chosen(name: Option<&str>, level: Option<ProxyLevel>) -> ChosenProxy {
        ChosenProxy { name: name.map(str::to_owned), level }
    }

    #[test]
    fn a_proxy_comes_from_the_account_then_the_provider_then_all_and_none_is_an_answer() {
        let (_dir, reg) = registry(
            "[connection]\nproxy = \"all-px\"\n\n[provider.openrouter.connection]\nproxy = \"prov-px\"\n\n[provider.anthropic.connection]\nproxy = \"none\"\n",
        );
        let accts = accounts(
            "schema = 2\n\
             [[account]]\nprovider = \"openrouter\"\nname = \"a\"\nsecret = \"sk-test-aaaa\"\nproxy = \"acct-px\"\n\
             [[account]]\nprovider = \"openrouter\"\nname = \"b\"\nsecret = \"sk-test-bbbb\"\n\
             [[account]]\nprovider = \"openrouter\"\nname = \"c\"\nsecret = \"sk-test-cccc\"\nproxy = \"none\"\n\
             [[account]]\nprovider = \"anthropic\"\nname = \"x\"\nsecret = \"sk-test-xxxx\"\n\
             [[account]]\nprovider = \"openai\"\nname = \"y\"\nsecret = \"sk-test-yyyy\"\n",
        );
        let of = |p: &str, n: &str| proxy_for(&reg, p, accts.get(p, n));
        assert_eq!(of("openrouter", "a"), chosen(Some("acct-px"), Some(ProxyLevel::Account)));
        assert_eq!(of("openrouter", "b"), chosen(Some("prov-px"), Some(ProxyLevel::Provider)));
        assert_eq!(of("openrouter", "c"), chosen(None, Some(ProxyLevel::Account)), "none at the account beats the provider");
        assert_eq!(of("anthropic", "x"), chosen(None, Some(ProxyLevel::Provider)), "none at the provider beats all");
        assert_eq!(of("openai", "y"), chosen(Some("all-px"), Some(ProxyLevel::All)));

        let (_dir, bare) = registry("");
        assert_eq!(proxy_for(&bare, "openai", None), chosen(None, None));
    }
}
