//! `$NULLROUTER_HOME/config.toml` (contracts/operator-config.md).

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;
use serde::de::{Deserializer, Error as _};

use super::duration::{de_duration, parse_duration};
use super::enums::ModelKind;
use super::primitives::BreakBehaviour;
use crate::validate::FieldPath;

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorConfig {
    pub schema: Option<i64>,
    #[serde(default)]
    pub unified_model: Vec<UnifiedModelDecl>,
    /// `[[combo]]` (spec 011): ordered fallback chains of unified models or other combos.
    #[serde(default)]
    pub combo: Vec<ComboDecl>,
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
    /// `[tests]` (spec 011): retest schedule, test timeouts and the test-call limit.
    #[serde(default)]
    pub tests: TestSettings,
}

/// `[tests]` (spec 011 data-model § Test settings). Serde parses the durations; the range rules
/// are in [`TestSettings::check`], so a load reports them at their position.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestSettings {
    /// Waits after an UNKNOWN; the last one repeats until the verdict settles.
    #[serde(default = "default_retest", deserialize_with = "de_durations")]
    pub retest: Vec<Duration>,
    /// How often a BROKEN verdict from a test is retested; `None` is `"off"`.
    #[serde(default, deserialize_with = "de_broken_retest")]
    pub broken_retest: Option<Duration>,
    /// Test calls at once, retests included.
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,
    #[serde(default)]
    pub timeout: TestTimeouts,
}

impl Default for TestSettings {
    fn default() -> Self {
        Self {
            retest: default_retest(),
            broken_retest: None,
            concurrency: default_concurrency(),
            timeout: TestTimeouts::default(),
        }
    }
}

/// `"on"`: a BROKEN verdict is retested once a day.
pub const BROKEN_RETEST_ON: Duration = Duration::from_secs(24 * 3600);

fn default_retest() -> Vec<Duration> {
    [60, 300, 1800, 6 * 3600].into_iter().map(Duration::from_secs).collect()
}

fn default_concurrency() -> u32 {
    4
}

fn de_durations<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Duration>, D::Error> {
    Vec::<String>::deserialize(d)?.iter().map(|s| parse_duration(s).map_err(D::Error::custom)).collect()
}

fn de_broken_retest<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
    parse_broken_retest(&String::deserialize(d)?).map_err(D::Error::custom)
}

/// `"off"`, `"on"` or a duration, as `[tests] broken_retest` takes it. The 1 h floor is checked
/// by [`TestSettings::check`].
pub fn parse_broken_retest(s: &str) -> Result<Option<Duration>, String> {
    match s {
        "off" => Ok(None),
        "on" => Ok(Some(BROKEN_RETEST_ON)),
        _ => parse_duration(s).map(Some).map_err(|_| "\"off\", \"on\" or at least 1h".to_owned()),
    }
}

/// The model types a test timeout is set for.
pub const TEST_TYPES: [&str; 6] = ["text", "embedding", "tts", "stt", "image", "video"];

/// `[tests.timeout]`: how long one test call of each type may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestTimeouts {
    pub text: Duration,
    pub embedding: Duration,
    pub tts: Duration,
    pub stt: Duration,
    pub image: Duration,
    pub video: Duration,
}

impl Default for TestTimeouts {
    fn default() -> Self {
        let s30 = Duration::from_secs(30);
        let m5 = Duration::from_secs(300);
        Self { text: s30, embedding: s30, tts: s30, stt: s30, image: m5, video: m5 }
    }
}

impl TestTimeouts {
    /// The timeout for a model of `kind`; an untyped model or any other kind is tested as text.
    pub fn for_kind(&self, kind: Option<ModelKind>) -> Duration {
        match kind {
            Some(ModelKind::Embedding) => self.embedding,
            Some(ModelKind::Tts) => self.tts,
            Some(ModelKind::Stt) => self.stt,
            Some(ModelKind::Image) => self.image,
            Some(ModelKind::Video) => self.video,
            _ => self.text,
        }
    }

    /// The timeout named by one of [`TEST_TYPES`].
    pub fn get_mut(&mut self, ty: &str) -> Option<&mut Duration> {
        Some(match ty {
            "text" => &mut self.text,
            "embedding" => &mut self.embedding,
            "tts" => &mut self.tts,
            "stt" => &mut self.stt,
            "image" => &mut self.image,
            "video" => &mut self.video,
            _ => return None,
        })
    }

    fn named(&self) -> [(&'static str, Duration); 6] {
        [
            ("text", self.text),
            ("embedding", self.embedding),
            ("tts", self.tts),
            ("stt", self.stt),
            ("image", self.image),
            ("video", self.video),
        ]
    }
}

impl<'de> Deserialize<'de> for TestTimeouts {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut out = Self::default();
        for (ty, v) in BTreeMap::<String, String>::deserialize(d)? {
            let slot = out.get_mut(&ty).ok_or_else(|| D::Error::custom(format!("unknown type {ty:?}")))?;
            *slot = parse_duration(&v).map_err(|e| D::Error::custom(format!("{ty}: {e}")))?;
        }
        Ok(out)
    }
}

impl TestSettings {
    /// The range rules of contracts/config-and-plugins.md, as `(path under tests, rule)`.
    pub fn check(&self) -> Vec<(FieldPath, String)> {
        let at = FieldPath::of("tests");
        let mut out = Vec::new();
        if self.retest.is_empty() || self.retest.len() > 10 {
            out.push((at.key("retest"), "1 to 10 steps".into()));
        }
        for (k, step) in self.retest.iter().enumerate() {
            if *step < Duration::from_secs(30) {
                out.push((at.key("retest").index(k), "at least 30s".into()));
            } else if k > 0 && *step < self.retest[k - 1] {
                out.push((at.key("retest").index(k), "steps must not get shorter".into()));
            }
        }
        if self.broken_retest.is_some_and(|d| d < Duration::from_secs(3600)) {
            out.push((at.key("broken_retest"), "\"off\", \"on\" or at least 1h".into()));
        }
        if !(1..=32).contains(&self.concurrency) {
            out.push((at.key("concurrency"), "1 to 32".into()));
        }
        for (ty, d) in self.timeout.named() {
            if d < Duration::from_secs(5) || d > Duration::from_secs(1800) {
                out.push((at.key("timeout").key(ty), "5s to 30m".into()));
            }
        }
        out
    }
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

/// A `[[combo]]` (spec 011 data-model § Combo): members are unified model or combo names,
/// tried in order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComboDecl {
    pub name: String,
    pub members: Vec<String>,
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
    fn test_settings() {
        let c: OperatorConfig = toml::from_str("").unwrap();
        assert_eq!(c.tests, TestSettings::default());
        assert_eq!(c.tests.retest.len(), 4);
        assert_eq!(c.tests.timeout.for_kind(Some(ModelKind::Video)), Duration::from_secs(300));
        assert_eq!(c.tests.timeout.for_kind(None), Duration::from_secs(30));
        assert!(c.tests.check().is_empty());
        let c: OperatorConfig =
            toml::from_str("[tests]\nbroken_retest = \"on\"\n[tests.timeout]\nimage = \"10m\"\n").unwrap();
        assert_eq!(c.tests.broken_retest, Some(BROKEN_RETEST_ON));
        assert_eq!(c.tests.timeout.image, Duration::from_secs(600));
        let err = toml::from_str::<OperatorConfig>("[tests.timeout]\naudio = \"1m\"\n").unwrap_err().to_string();
        assert!(err.contains("unknown type \"audio\""), "{err}");
        let err = toml::from_str::<OperatorConfig>("[tests]\nbroken_retest = \"sometimes\"\n").unwrap_err().to_string();
        assert!(err.contains("\"off\", \"on\" or at least 1h"), "{err}");
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
