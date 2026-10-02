//! A provider's session header, derived from the agent id (contracts/provider-schema-v2.md
//! § Session).

use serde::Deserialize;

use super::primitives::SessionDerive;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSession {
    pub header: String,
    pub derive: SessionDerive,
}
