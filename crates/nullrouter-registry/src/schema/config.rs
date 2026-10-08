//! `$NULLROUTER_HOME/config.toml` (contracts/operator-config.md).

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;
use serde::de::{Deserializer, Error as _};

use super::duration::{de_duration, parse_duration};
use super::enums::ModelKind;
use super::primitives::BreakBehaviour;

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorConfig {
    pub schema: Option<i64>,
    #[serde(default)]
    pub unified_model: Vec<UnifiedModelDecl>,
    #[serde(default)]
    pub provider: BTreeMap<String, ProviderSettings>,
    #[serde(default)]
    pub plugin_decisions: BTreeMap<String, Decision>,
    /// Lets plugin endpoints reach loopback, private and link-local hosts (research R18).
    #[serde(default)]
    pub allow_private_endpoints: bool,
    #[serde(default)]
    pub server: ServerSettings,
    #[serde(default)]
    pub pipeline: PipelineSettings,
    #[serde(default)]
    pub routing: RoutingSettings,
    #[serde(default)]
    pub dashboard: DashboardSettings,
    #[serde(default)]
    pub adapters: AdaptersSettings,
}

/// `[adapters]` (spec 004): where the catalogue and builder are, and the sandbox's limits.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptersSettings {
    /// HTTPS URL of the catalogue index. The SSRF rules are applied when it is fetched.
    #[serde(default = "default_catalogue_url", deserialize_with = "de_https_url")]
    pub catalogue_url: String,
    /// Path to the builder binary. Absent means `nullrouter-builder` on `PATH`, and installs
    /// stop at `queued` if that isn't there either.
    #[serde(default)]
    pub builder: Option<String>,
    #[serde(default = "default_request_deadline", deserialize_with = "de_request_deadline")]
    pub request_deadline_ms: u32,
    #[serde(default = "default_event_deadline", deserialize_with = "de_event_deadline")]
    pub event_deadline_ms: u32,
    #[serde(default = "default_memory", deserialize_with = "de_memory")]
    pub memory_mib: u32,
    /// The pooling allocator's instance count.
    #[serde(default = "default_instances", deserialize_with = "de_instances")]
    pub max_instances: u32,
}

impl Default for AdaptersSettings {
    fn default() -> Self {
        Self {
            catalogue_url: default_catalogue_url(),
            builder: None,
            request_deadline_ms: default_request_deadline(),
            event_deadline_ms: default_event_deadline(),
            memory_mib: default_memory(),
            max_instances: default_instances(),
        }
    }
}

fn default_catalogue_url() -> String {
    "https://raw.githubusercontent.com/NullCombinator/0xRouter/main/catalogue/index.toml".into()
}
fn default_request_deadline() -> u32 {
    20
}
fn default_event_deadline() -> u32 {
    2
}
fn default_memory() -> u32 {
    64
}
fn default_instances() -> u32 {
    64
}

fn de_https_url<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    if s.starts_with("https://") && s.len() > "https://".len() {
        Ok(s)
    } else {
        Err(D::Error::custom("catalogue_url must be an https:// URL"))
    }
}

fn in_range<'de, D: Deserializer<'de>>(d: D, name: &str, lo: u32, hi: u32) -> Result<u32, D::Error> {
    let v = u32::deserialize(d)?;
    if (lo..=hi).contains(&v) { Ok(v) } else { Err(D::Error::custom(format!("{name} must be {lo} to {hi}"))) }
}

fn de_request_deadline<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    in_range(d, "request_deadline_ms", 1, 1000)
}
fn de_event_deadline<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    in_range(d, "event_deadline_ms", 1, 100)
}
fn de_memory<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    in_range(d, "memory_mib", 1, 512)
}
fn de_instances<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    in_range(d, "max_instances", 1, 4096)
}

/// `[dashboard]` (spec 009): whether `serve` also serves the read-only dashboard, and where.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardSettings {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// `host:port`. The host must be a loopback address (`127.0.0.0/8`, `::1`, `localhost`).
    #[serde(default = "default_dashboard_listen", deserialize_with = "de_loopback_listen")]
    pub listen: String,
}

impl Default for DashboardSettings {
    fn default() -> Self {
        Self { enabled: true, listen: default_dashboard_listen() }
    }
}

fn default_dashboard_listen() -> String {
    "127.0.0.1:20130".into()
}

/// Whether `host` (as written in a `host:port` listen address) is a loopback address.
fn is_loopback_host(host: &str) -> bool {
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if bare.eq_ignore_ascii_case("localhost") {
        return true;
    }
    bare.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn de_loopback_listen<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let listen = String::deserialize(d)?;
    let Some((host, port)) = listen.rsplit_once(':') else {
        return Err(D::Error::custom(format!("{listen:?} is not host:port")));
    };
    if port.parse::<u16>().is_err() {
        return Err(D::Error::custom(format!("{listen:?} has no valid port")));
    }
    if !is_loopback_host(host) {
        return Err(D::Error::custom("must be a loopback address; network binding is not supported"));
    }
    Ok(listen)
}

/// `[routing]`: how long the amortization window is (Clarifications Q4). Cold work is spread over
/// a window aligned to the Unix epoch; the length is the default or a target's own.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingSettings {
    #[serde(default = "five_hours", deserialize_with = "de_positive_duration")]
    pub amortization: Duration,
    /// Target (a unified model name or `provider/model`) → window length.
    #[serde(default, deserialize_with = "de_duration_map")]
    pub amortization_for: BTreeMap<String, Duration>,
}

impl Default for RoutingSettings {
    fn default() -> Self {
        Self { amortization: five_hours(), amortization_for: BTreeMap::new() }
    }
}

fn five_hours() -> Duration {
    Duration::from_secs(5 * 3600)
}

fn de_positive_duration<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
    let v = de_duration(d)?;
    if v.is_zero() { Err(D::Error::custom("an amortization window must be more than 0")) } else { Ok(v) }
}

fn de_duration_map<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, Duration>, D::Error> {
    BTreeMap::<String, String>::deserialize(d)?
        .into_iter()
        .map(|(k, v)| match parse_duration(&v) {
            Ok(v) if !v.is_zero() => Ok((k, v)),
            Ok(_) => Err(D::Error::custom(format!("{k:?}: an amortization window must be more than 0"))),
            Err(e) => Err(D::Error::custom(format!("{k:?}: {e}"))),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSettings {
    #[serde(default = "default_listen")]
    pub listen: String,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self { listen: default_listen() }
    }
}

fn default_listen() -> String {
    "127.0.0.1:20129".into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineSettings {
    /// What a stream that breaks after output does, unless the agent key overrides it.
    #[serde(default = "restart")]
    pub break_behaviour: BreakBehaviour,
}

impl Default for PipelineSettings {
    fn default() -> Self {
        Self { break_behaviour: restart() }
    }
}

fn restart() -> BreakBehaviour {
    BreakBehaviour::Restart
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnifiedModelDecl {
    pub name: String,
    pub kind: Option<ModelKind>,
    pub members: Vec<MemberDecl>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberDecl {
    pub provider: String,
    pub model: String,
}

/// Operator settings for one provider. Keyed by provider id, so they survive a replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSettings {
    #[serde(default = "yes")]
    pub allow_uncatalogued_models: bool,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self { allow_uncatalogued_models: true }
    }
}

fn yes() -> bool {
    true
}

/// Operator decision on a user plugin that shadows a bundled id (FR-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Replace,
    Decline,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_defaults() {
        let c: OperatorConfig = toml::from_str("").unwrap();
        assert!(!c.allow_private_endpoints);
        assert_eq!(c.server.listen, "127.0.0.1:20129");
        assert_eq!(c.pipeline.break_behaviour, BreakBehaviour::Restart);
    }

    #[test]
    fn adapters_defaults_and_ranges() {
        let c: OperatorConfig = toml::from_str("").unwrap();
        let a = &c.adapters;
        assert_eq!((a.request_deadline_ms, a.event_deadline_ms, a.memory_mib, a.max_instances), (20, 2, 64, 64));
        assert!(a.builder.is_none() && a.catalogue_url.starts_with("https://"));
        let c: OperatorConfig =
            toml::from_str("[adapters]\nbuilder = \"/opt/nr-builder\"\nrequest_deadline_ms = 1000\nevent_deadline_ms = 100\nmemory_mib = 512\n")
                .unwrap();
        assert_eq!(c.adapters.builder.as_deref(), Some("/opt/nr-builder"));
        for bad in [
            "request_deadline_ms = 0",
            "request_deadline_ms = 1001",
            "event_deadline_ms = 101",
            "memory_mib = 0",
            "memory_mib = 513",
            "catalogue_url = \"http://example.com/index.toml\"",
            "surprise = 1",
        ] {
            assert!(toml::from_str::<OperatorConfig>(&format!("[adapters]\n{bad}\n")).is_err(), "{bad}");
        }
    }

    #[test]
    fn routing_settings() {
        let c: OperatorConfig = toml::from_str("").unwrap();
        assert_eq!(c.routing.amortization, Duration::from_secs(5 * 3600));
        let c: OperatorConfig = toml::from_str(
            "[routing]\namortization = \"2h\"\n[routing.amortization_for]\nsonnet = \"1h\"\n\"anthropic/claude-opus-4-1\" = \"24h\"\n",
        )
        .unwrap();
        assert_eq!(c.routing.amortization, Duration::from_secs(7200));
        assert_eq!(c.routing.amortization_for["sonnet"], Duration::from_secs(3600));
        for bad in [
            "[routing]\namortization = \"0s\"",
            "[routing]\namortization = \"soon\"",
            "[routing.amortization_for]\nsonnet = \"0m\"",
            "[routing]\nwindow = \"5h\"",
        ] {
            assert!(toml::from_str::<OperatorConfig>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn dashboard_settings() {
        let c: OperatorConfig = toml::from_str("").unwrap();
        assert!(c.dashboard.enabled);
        assert_eq!(c.dashboard.listen, "127.0.0.1:20130");
        for good in ["127.0.0.1:1", "127.8.9.10:20130", "[::1]:20130", "localhost:20130", "LOCALHOST:9"] {
            let c: OperatorConfig = toml::from_str(&format!("[dashboard]\nlisten = \"{good}\"\n")).unwrap();
            assert_eq!(c.dashboard.listen, good);
        }
        let c: OperatorConfig = toml::from_str("[dashboard]\nenabled = false\n").unwrap();
        assert!(!c.dashboard.enabled);
        assert_eq!(c.dashboard.listen, "127.0.0.1:20130");
        for bad in
            ["0.0.0.0:20130", "[::]:20130", "192.168.1.5:20130", "example.com:20130", "10.0.0.1:1", "128.0.0.1:1"]
        {
            let err = toml::from_str::<OperatorConfig>(&format!("[dashboard]\nlisten = \"{bad}\"\n"))
                .unwrap_err()
                .to_string();
            assert!(err.contains("must be a loopback address; network binding is not supported"), "{bad}: {err}");
        }
        for bad in ["127.0.0.1", "127.0.0.1:http", "127.0.0.1:70000"] {
            assert!(toml::from_str::<OperatorConfig>(&format!("[dashboard]\nlisten = \"{bad}\"\n")).is_err(), "{bad}");
        }
        assert!(toml::from_str::<OperatorConfig>("[dashboard]\nport = 1\n").is_err());
    }

    #[test]
    fn pipeline_settings_parse() {
        let c: OperatorConfig = toml::from_str(
            "allow_private_endpoints = true\n[server]\nlisten = \"0.0.0.0:8080\"\n[pipeline]\nbreak_behaviour = \"error_event\"\n",
        )
        .unwrap();
        assert!(c.allow_private_endpoints);
        assert_eq!(c.server.listen, "0.0.0.0:8080");
        assert_eq!(c.pipeline.break_behaviour, BreakBehaviour::ErrorEvent);
        let err =
            toml::from_str::<OperatorConfig>("[pipeline]\nbreak_behaviour = \"retry\"\n").unwrap_err().to_string();
        assert!(err.contains("unknown break behaviour \"retry\""), "{err}");
    }
}
