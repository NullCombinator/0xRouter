//! `[models_live]`: a provider's live model list, read with a sign-in account (research
//! R14). Listed models join the static `[[models]]`, which stay the fallback.

use std::time::Duration;

use indexmap::IndexMap;
use serde::Deserialize;
use serde::de::Deserializer;

use super::duration::de_duration;
use super::primitives::ModelType;
use super::quota::ValuePath;

/// A listed model's type: a fixed model type, or a path read from each entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveModelType {
    Fixed(ModelType),
    Path(ValuePath),
}

impl Default for LiveModelType {
    fn default() -> Self {
        Self::Fixed(ModelType::Text)
    }
}

impl<'de> Deserialize<'de> for LiveModelType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match ModelType::parse(&s) {
            Some(t) => Ok(Self::Fixed(t)),
            None => ValuePath::parse(&s).map(Self::Path).map_err(serde::de::Error::custom),
        }
    }
}

/// `[models_live]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelsLiveDecl {
    pub url: String,
    /// Static, non-secret headers. The account's credential is added by the core.
    #[serde(default)]
    pub headers: IndexMap<String, String>,
    /// Where the list sits; `.` is the root.
    pub list: ValuePath,
    pub id: ValuePath,
    pub name: Option<ValuePath>,
    pub context: Option<ValuePath>,
    pub max_output: Option<ValuePath>,
    #[serde(default, rename = "type")]
    pub model_type: LiveModelType,
    #[serde(default = "six_hours", deserialize_with = "de_duration")]
    pub refresh: Duration,
}

fn six_hours() -> Duration {
    Duration::from_secs(6 * 3600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_contract_example() {
        let d: ModelsLiveDecl = toml::from_str(
            r#"
url = "https://cli-chat-proxy.grok.com/v1/models"
headers = { x-xai-token-auth = "xai-grok-cli", x-grok-client-mode = "headless" }
list = "data | models | results | ."
id = "id | model_id | modelId | slug | name"
name = "display_name | displayName | name"
context = "context_length | context_window"
max_output = "max_output_tokens"
type = "text"
refresh = "6h"
"#,
        )
        .unwrap();
        assert_eq!(d.list.0.last().map(String::as_str), Some("."));
        assert_eq!(d.model_type, LiveModelType::Fixed(ModelType::Text));
        assert_eq!(d.refresh, six_hours());
        let d: ModelsLiveDecl =
            toml::from_str("url = \"https://a.example\"\nlist = \".\"\nid = \"id\"\ntype = \"kind\"\n").unwrap();
        assert_eq!(d.model_type, LiveModelType::Path(ValuePath(vec!["kind".into()])));
        assert_eq!(d.refresh, six_hours());
    }
}
