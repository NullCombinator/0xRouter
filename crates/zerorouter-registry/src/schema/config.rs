//! `$ZEROROUTER_HOME/config.toml` (contracts/operator-config.md).

use std::collections::BTreeMap;

use serde::Deserialize;

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
