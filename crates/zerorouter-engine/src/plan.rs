//! The per-request candidate order (data-model § RequestPlan).
//!
//! A candidate is one provider, one endpoint, one account and one upstream model id.
//! Within a provider the endpoint speaking the client's style comes first (research R27),
//! then the model's `wires` in declared order, then the provider's other endpoints for
//! the type in declared order.

use zerorouter_registry::schema::{Endpoint, ModelType, ProviderEntity};
use zerorouter_registry::{NotFound, Registry, Resolution};

use crate::accounts::{Account, Accounts};

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

#[derive(Debug, Clone)]
pub struct RequestPlan<'s> {
    pub candidates: Vec<Candidate<'s>>,
    /// The unified model's name when the target was one.
    pub unified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error(transparent)]
    NotFound(#[from] NotFound),
    #[error("provider {provider} has no {ty} endpoint for model {model}")]
    NoEndpoint { provider: String, ty: ModelType, model: String },
    #[error("no enabled account for provider {provider}; add one with `zerorouter accounts add {provider} <name>`")]
    NoAccount { provider: String },
}

impl PlanError {
    /// The HTTP status the client sees.
    pub fn status(&self) -> u16 {
        match self {
            Self::NotFound(_) | Self::NoEndpoint { .. } => 404,
            Self::NoAccount { .. } => 503,
        }
    }
}

/// `p`'s endpoints for `ty` that serve `upstream_id`, in try order for a client speaking
/// `client_style`. `wires` is the model's declared list, if any.
pub fn endpoints<'p>(p: &'p ProviderEntity, ty: ModelType, upstream_id: &str, wires: Option<&[String]>, client_style: &str) -> Vec<&'p Endpoint> {
    let all: Vec<&Endpoint> = p.endpoints.get(&ty).map_or(&[][..], |e| &e.0[..]).iter().filter(|e| e.serves(upstream_id)).collect();
    let allowed = |e: &&Endpoint| wires.is_none_or(|w| e.wire.as_ref().is_some_and(|x| w.contains(x)));
    let mut out: Vec<&Endpoint> = all.iter().copied().filter(|e| e.wire.as_deref() == Some(client_style)).filter(allowed).collect();
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

fn member<'s>(
    registry: &'s Registry,
    accounts: &'s Accounts,
    provider: &'s ProviderEntity,
    requested: &str,
    upstream_id: String,
    ty: ModelType,
    client_style: &str,
) -> Result<Candidate<'s>, PlanError> {
    let model = registry.model(&provider.id, requested).ok().and_then(|m| m.model);
    let wires = model.and_then(|m| m.wires.as_deref());
    let endpoint = endpoints(provider, ty, &upstream_id, wires, client_style).into_iter().next().ok_or_else(|| {
        PlanError::NoEndpoint { provider: provider.id.clone(), ty, model: requested.to_owned() }
    })?;
    let no_auth = provider.auth.as_ref().is_some_and(|a| a.no_auth);
    let account = if no_auth {
        None
    } else {
        Some(accounts.for_provider(&provider.id).next().ok_or_else(|| PlanError::NoAccount { provider: provider.id.clone() })?)
    };
    Ok(Candidate { provider, endpoint, account, requested: requested.to_owned(), upstream_id })
}

/// The happy-path plan: one candidate, the first account in operator order. A unified
/// target takes its first member that has an endpoint and an account.
pub fn plan<'s>(
    registry: &'s Registry,
    accounts: &'s Accounts,
    target: &'s str,
    ty: ModelType,
    client_style: &str,
) -> Result<RequestPlan<'s>, PlanError> {
    match registry.resolve(target)? {
        Resolution::Direct { provider, requested, upstream_id, .. } => {
            let c = member(registry, accounts, provider, requested, upstream_id, ty, client_style)?;
            Ok(RequestPlan { candidates: vec![c], unified: None })
        }
        Resolution::Unified(u) => {
            let mut first_err = None;
            for m in &u.members {
                let provider = registry.provider(&m.provider)?;
                match member(registry, accounts, provider, &m.requested, m.upstream_id.clone(), ty, client_style) {
                    Ok(c) => return Ok(RequestPlan { candidates: vec![c], unified: Some(u.name.clone()) }),
                    Err(e) => {
                        first_err.get_or_insert(e);
                    }
                }
            }
            Err(first_err.unwrap_or(PlanError::NotFound(NotFound::UnifiedModel { name: u.name.clone() })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(src: &str) -> ProviderEntity {
        zerorouter_registry::validate_user_plugin(src, std::path::Path::new("acme.toml")).unwrap_or_else(|e| panic!("{e:#?}"))
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
        assert_eq!(wires(&endpoints(&p, t, "only-this", None, "openai-responses")), ["openai-responses", "openai-chat", "anthropic-messages"]);
        let w = ["anthropic-messages".to_owned()];
        assert_eq!(wires(&endpoints(&p, t, "m", Some(&w), "openai-chat")), ["anthropic-messages"], "wires restrict the endpoints");
        let w = ["openai-chat".to_owned(), "anthropic-messages".to_owned()];
        assert_eq!(wires(&endpoints(&p, t, "m", Some(&w), "anthropic-messages")), ["anthropic-messages", "openai-chat"]);
        assert_eq!(wires(&endpoints(&p, t, "m", Some(&w), "gemini")), ["openai-chat", "anthropic-messages"]);
        assert!(endpoints(&p, ModelType::Tts, "m", None, "openai-chat").is_empty());
    }
}
