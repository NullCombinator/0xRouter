//! `$ZEROROUTER_HOME/config.toml` (contracts/operator-config.md).

use std::collections::BTreeMap;

use serde::Deserialize;

use super::enums::ModelKind;

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
