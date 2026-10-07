//! Serde types for plugin files and operator config. Every struct rejects unknown keys.

mod capability;
mod config;
mod duration;
mod endpoint;
mod enums;
mod forwarding;
mod identity;
mod model;
mod models_live;
mod oauth;
mod oauth_params;
mod plugin;
mod primitives;
mod quota;
mod rejection;
mod routing;
mod section_formats;
mod session;
mod signin;
mod style;
mod transport;

pub use capability::{CapabilitySection, ModelRoute, SectionEndpoint, SectionLimits, SectionModel};
pub use config::{
    BROKEN_RETEST_ON, DashboardSettings, Decision, MemberDecl, OperatorConfig, PipelineSettings, ProviderSettings,
    RoutingSettings, ServerSettings, TEST_TYPES, TestSettings, TestTimeouts, UnifiedModelDecl, parse_broken_retest,
};
pub use duration::parse_duration;
pub use endpoint::{
    Continuation, Endpoint, EndpointAuth, Endpoints, ErrorRule, ErrorRules, ForceMap, ForcedParam, JobMapping,
    JobState, RetryOverride, TokenCount,
};
pub use enums::{
    AuthHook, AuthKind, AuthScheme, CapabilityKind, Category, ContentKind, ModelKind, Quirk, RejectionReason, WireFormat,
};
pub use forwarding::{ForwardHeader, Forwarding, ToClient, ToUpstream};
pub use identity::{HeaderValue, IdentityDecl, Placeholder};
pub use model::Model;
pub use models_live::{LiveModelType, ModelsLiveDecl};
pub(crate) use oauth::oauth_urls;
pub use oauth::{OAuthDecl, ParamValue, host_set};
pub use oauth_params::KNOWN_OAUTH_PARAMS;
pub use plugin::{
    AuthDecl, Display, Features, ModelsFetcher, Notice, PluginFile, PluginSource, ProviderEntity, Region,
    ThinkingDisplay,
};
pub(crate) use plugin::{account_urls, endpoint_hosts};
pub use primitives::*;
pub use quota::{
    QuotaAccounts, QuotaBody, QuotaDecl, QuotaDecoder, QuotaRequest, QuotaSource, QuotaUnit, ResetsFormat, ValuePath,
    WindowRule,
};
pub use rejection::RejectionRule;
pub use routing::*;
pub use section_formats::KNOWN_SECTION_FORMATS;
pub use session::ProviderSession;
pub use signin::{
    DEFAULT_VERIFIER_BYTES, Redirect, RedirectKind, RefusedRule, SignInDecl, SignInFlow, SignInParam, SignInParamValue,
    SignInProfile, TokenBody,
};
pub use style::*;
pub use transport::{
    AuthPlacement, CopilotParams, ExecutorParams, RetryPolicy, StringOrList, Transport, TransportAuth,
};
