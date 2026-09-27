//! Serde types for plugin files and operator config. Every struct rejects unknown keys.

mod capability;
mod config;
mod enums;
mod model;
mod oauth;
mod oauth_params;
mod plugin;
mod section_formats;
mod transport;

pub use capability::{CapabilitySection, ModelRoute, SectionEndpoint, SectionLimits, SectionModel};
pub use config::{Decision, MemberDecl, OperatorConfig, ProviderSettings, UnifiedModelDecl};
pub use enums::{AuthHook, AuthKind, AuthScheme, CapabilityKind, Category, ContentKind, ModelKind, Quirk, WireFormat};
pub use model::Model;
pub(crate) use oauth::oauth_urls;
pub use oauth::{OAuthDecl, ParamValue, host_set};
pub use oauth_params::KNOWN_OAUTH_PARAMS;
pub use plugin::{
    AuthDecl, Display, Features, ModelsFetcher, Notice, PluginFile, PluginSource, ProviderEntity, Region,
    ThinkingDisplay,
};
pub use section_formats::KNOWN_SECTION_FORMATS;
pub use transport::{
    AuthPlacement, CopilotParams, ExecutorParams, RetryPolicy, StringOrList, Transport, TransportAuth,
};
