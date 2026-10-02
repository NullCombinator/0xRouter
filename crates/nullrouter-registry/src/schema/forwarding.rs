//! Header and body forwarding between client and upstream (contracts/provider-schema-v2.md
//! § Forwarding). Every entry is checked against the security floor at load.

use serde::Deserialize;

use super::primitives::ForwardMerge;

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forwarding {
    #[serde(default)]
    pub to_upstream: ToUpstream,
    #[serde(default)]
    pub to_client: ToClient,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToUpstream {
    #[serde(default)]
    pub headers: Vec<ForwardHeader>,
}

/// A client header passed to the upstream.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardHeader {
    pub name: String,
    #[serde(default = "replace")]
    pub merge: ForwardMerge,
    /// Only when the client used one of these styles; empty = any.
    #[serde(default)]
    pub from_styles: Vec<String>,
}

fn replace() -> ForwardMerge {
    ForwardMerge::Replace
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToClient {
    /// Header names or trailing-`*` patterns.
    #[serde(default)]
    pub headers: Vec<String>,
    /// Response body paths copied verbatim; native pairs only.
    #[serde(default)]
    pub body: Vec<String>,
}
