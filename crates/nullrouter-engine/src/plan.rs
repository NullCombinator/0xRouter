//! The per-request candidate order (data-model § RequestPlan).
//!
//! A candidate is one provider, one endpoint, one account and one upstream model id.
//! Within a provider the endpoint speaking the client's style comes first (research R27),
//! then the model's `wires` in declared order, then the provider's other endpoints for
//! the type in declared order.

use std::collections::HashMap;
use std::sync::Mutex;

use nullrouter_registry::schema::{Endpoint, ModelType, ProviderEntity};
use nullrouter_registry::{NotFound, Registry, Resolution};

use crate::accounts::{self, Account, Accounts};
use crate::keys::AgentId;
use crate::records::ErrorClass;
use crate::tokens::TokenCells;

#[derive(Debug, Clone)]
pub struct Candidate<'s> {
    pub provider: &'s ProviderEntity,
    pub endpoint: &'s Endpoint,
    /// `None` for a provider with `auth.no_auth`.
    pub account: Option<&'s Account>,
    /// The model as the target named it.
    pub requested: String,
    pub upstream_id: String,
}

impl Candidate<'_> {
    /// Whether this attempt speaks the client's style (research R27).
    pub fn same_style(&self, client_style: &str) -> bool {
        self.endpoint.wire.as_deref() == Some(client_style)
    }
}

/// A plan entry that can't be tried, recorded as a `skipped` attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub provider: String,
    pub account: Option<String>,
    pub model: String,
    pub reason: String,
    /// Set for out-of-service sign-in accounts (`NeedsSignIn`, `Refused`,
    /// `TokenRefreshing`).
    pub class: Option<ErrorClass>,
}

#[derive(Debug, Clone)]
pub enum Step<'s> {
    Try(Candidate<'s>),
    Skip(Skip),
}

#[derive(Debug, Clone)]
pub struct RequestPlan<'s> {
    pub steps: Vec<Step<'s>>,
    /// The unified model's name when the target was one.
    pub unified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error(transparent)]
    NotFound(#[from] NotFound),
    #[error("provider {provider} has no {ty} endpoint for model {model}")]
    NoEndpoint { provider: String, ty: ModelType, model: String },
    #[error("no enabled account for provider {provider}; add one with `nullrouter accounts add {provider} <name>`")]
    NoAccount { provider: String },
    /// FR-012: the route's type isn't the model's declared kind.
    #[error("{target} is a {model} model, and this route takes {route} models")]
    TypeMismatch { target: String, model: ModelType, route: ModelType },
}

impl PlanError {
    /// The HTTP status the client sees.
    pub fn status(&self) -> u16 {
        match self {
            Self::NotFound(_) | Self::NoEndpoint { .. } => 404,
            Self::NoAccount { .. } => 503,
            Self::TypeMismatch { .. } => 400,
        }
    }
}

/// The account an agent was last served by for a target (research R8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warm {
    pub provider: String,
    /// `None` for a provider with `auth.no_auth`.
    pub account: Option<String>,
}

/// `(agent, target) → the account that last completed a request`. Updated on successful
/// completion only (a stream counts when it ends); last success wins. Held in memory.
#[derive(Debug, Default)]
pub struct WarmMap {
    inner: Mutex<HashMap<(AgentId, String), Warm>>,
}

impl WarmMap {
    pub fn get(&self, agent: &AgentId, target: &str) -> Option<Warm> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).get(&(agent.clone(), target.to_owned())).cloned()
    }

    pub fn set(&self, agent: &AgentId, target: &str, warm: Warm) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).insert((agent.clone(), target.to_owned()), warm);
    }
}

/// `p`'s endpoints for `ty` that serve `upstream_id`, in try order for a client speaking
/// `client_style`. `wires` is the model's declared list, if any.
pub fn endpoints<'p>(
    p: &'p ProviderEntity,
    ty: ModelType,
    upstream_id: &str,
    wires: Option<&[String]>,
    client_style: &str,
) -> Vec<&'p Endpoint> {
    let all: Vec<&Endpoint> =
        p.endpoints.get(&ty).map_or(&[][..], |e| &e.0[..]).iter().filter(|e| e.serves(upstream_id)).collect();
    let allowed = |e: &&Endpoint| wires.is_none_or(|w| e.wire.as_ref().is_some_and(|x| w.contains(x)));
    let mut out: Vec<&Endpoint> =
        all.iter().copied().filter(|e| e.wire.as_deref() == Some(client_style)).filter(allowed).collect();
    for w in wires.into_iter().flatten() {
        out.extend(all.iter().copied().filter(|e| e.wire.as_ref() == Some(w)));
    }
    if wires.is_none() {
        out.extend(all.iter().copied());
    }
    let mut seen = Vec::new();
    out.retain(|e| {
        let new = !seen.contains(&std::ptr::from_ref(*e));
        seen.push(std::ptr::from_ref(*e));
        new
    });
    out
}

/// What every member of one request is planned for.
#[derive(Clone, Copy)]
struct Ask<'a> {
    ty: ModelType,
    client_style: &'a str,
    warm: Option<&'a Warm>,
    tokens: &'a TokenCells,
}

/// One member's steps: its accounts in operator order, the warm one first, or one skip.
/// A sign-in account that can't serve now is a recorded skip naming the command that
/// brings it back (research R10, FR-016); a disabled account is left out silently.
fn member<'s>(
    registry: &'s Registry,
    accounts: &'s Accounts,
    provider: &'s ProviderEntity,
    requested: &str,
    upstream_id: String,
    ask: &Ask,
) -> Result<Vec<Step<'s>>, PlanError> {
    let Ask { ty, client_style, warm, tokens } = *ask;
    let model = registry.model(&provider.id, requested).ok().and_then(|m| m.model);
    let wires = model.and_then(|m| m.wires.as_deref());
    let endpoint = endpoints(provider, ty, &upstream_id, wires, client_style)
        .into_iter()
        .next()
        .ok_or_else(|| PlanError::NoEndpoint { provider: provider.id.clone(), ty, model: requested.to_owned() })?;
    let candidate = |account| {
        Step::Try(Candidate {
            provider,
            endpoint,
            account,
            requested: requested.to_owned(),
            upstream_id: upstream_id.clone(),
        })
    };
    if provider.auth.as_ref().is_some_and(|a| a.no_auth) {
        return Ok(vec![candidate(None)]);
    }
    let mut mine: Vec<&Account> = accounts.for_provider(&provider.id).collect();
    if mine.is_empty() {
        return Err(PlanError::NoAccount { provider: provider.id.clone() });
    }
    if let Some(w) = warm.filter(|w| w.provider == provider.id)
        && let Some(i) = mine.iter().position(|a| w.account.as_deref() == Some(a.name.as_str()))
    {
        let a = mine.remove(i);
        mine.insert(0, a);
    }
    Ok(mine
        .into_iter()
        .map(|a| match accounts::out_of_service(a, tokens) {
            None => candidate(Some(a)),
            Some(w) => Step::Skip(Skip {
                provider: provider.id.clone(),
                account: Some(a.name.clone()),
                model: upstream_id.clone(),
                reason: w.to_string(),
                class: w.class(),
            }),
        })
        .collect())
}

/// The candidate order for one request (research R7, R8): the warm account first, then its
/// provider's other accounts in operator order, then, for a unified target, the other
/// members in declared order, each with its accounts. A member that isn't installed, has
/// no endpoint for the type or has no account is a skip. `warm` is left out by the caller
/// when that account is cooling.
pub fn plan<'s>(
    registry: &'s Registry,
    accounts: &'s Accounts,
    tokens: &TokenCells,
    target: &'s str,
    ty: ModelType,
    client_style: &str,
    warm: Option<&Warm>,
) -> Result<RequestPlan<'s>, PlanError> {
    let resolution = registry.resolve(target)?;
    let declared = match &resolution {
        Resolution::Direct { provider, requested, .. } => {
            registry.model(&provider.id, requested).ok().and_then(|m| m.kind)
        }
        Resolution::Unified(u) => u.kind,
    };
    // An undeclared kind passes: the endpoint list decides.
    if let Some(model) = declared.and_then(ModelType::from_capability)
        && model != ty
    {
        return Err(PlanError::TypeMismatch { target: target.to_owned(), model, route: ty });
    }
    let ask = Ask { ty, client_style, warm, tokens };
    match resolution {
        Resolution::Direct { provider, requested, upstream_id, .. } => {
            let steps = member(registry, accounts, provider, requested, upstream_id, &ask)?;
            Ok(RequestPlan { steps, unified: None })
        }
        Resolution::Unified(u) => {
            let mut members: Vec<_> = u.members.iter().collect();
            if let Some(i) = warm.and_then(|w| members.iter().position(|m| m.provider == w.provider)) {
                let m = members.remove(i);
                members.insert(0, m);
            }
            let mut steps = Vec::new();
            for m in members {
                let skip = |reason: String| {
                    Step::Skip(Skip {
                        provider: m.provider.clone(),
                        account: None,
                        model: m.upstream_id.clone(),
                        reason,
                        class: None,
                    })
                };
                let Ok(provider) = registry.provider(&m.provider) else {
                    steps.push(skip(format!("provider {} isn't installed", m.provider)));
                    continue;
                };
                match member(registry, accounts, provider, &m.requested, m.upstream_id.clone(), &ask) {
                    Ok(s) => steps.extend(s),
                    Err(e) => steps.push(skip(e.to_string())),
                }
            }
            Ok(RequestPlan { steps, unified: Some(u.name.clone()) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(src: &str) -> ProviderEntity {
        nullrouter_registry::validate_user_plugin(src, std::path::Path::new("acme.toml"))
            .unwrap_or_else(|e| panic!("{e:#?}"))
    }

    const ACME: &str = r#"
schema = 2
id = "acme"
category = "apikey"
[[endpoints.text]]
url = "https://api.acme.example/v1/chat/completions"
wire = "openai-chat"
[[endpoints.text]]
url = "https://api.acme.example/v1/messages"
wire = "anthropic-messages"
[[endpoints.text]]
url = "https://api.acme.example/v1/responses"
wire = "openai-responses"
models = ["only-this"]
"#;

    fn wires(e: &[&Endpoint]) -> Vec<String> {
        e.iter().map(|e| e.wire.clone().unwrap_or_default()).collect()
    }

    #[test]
    fn the_native_pair_comes_first_then_the_models_wires() {
        let p = provider(ACME);
        let t = ModelType::Text;
        assert_eq!(wires(&endpoints(&p, t, "m", None, "anthropic-messages")), ["anthropic-messages", "openai-chat"]);
        assert_eq!(wires(&endpoints(&p, t, "m", None, "gemini")), ["openai-chat", "anthropic-messages"]);
        assert_eq!(
            wires(&endpoints(&p, t, "only-this", None, "openai-responses")),
            ["openai-responses", "openai-chat", "anthropic-messages"]
        );
        let w = ["anthropic-messages".to_owned()];
        assert_eq!(
            wires(&endpoints(&p, t, "m", Some(&w), "openai-chat")),
            ["anthropic-messages"],
            "wires restrict the endpoints"
        );
        let w = ["openai-chat".to_owned(), "anthropic-messages".to_owned()];
        assert_eq!(
            wires(&endpoints(&p, t, "m", Some(&w), "anthropic-messages")),
            ["anthropic-messages", "openai-chat"]
        );
        assert_eq!(wires(&endpoints(&p, t, "m", Some(&w), "gemini")), ["openai-chat", "anthropic-messages"]);
        assert!(endpoints(&p, ModelType::Tts, "m", None, "openai-chat").is_empty());
    }
}
