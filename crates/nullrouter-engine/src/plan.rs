//! The per-request candidate order (data-model § RequestPlan).
//!
//! A candidate is one provider, one endpoint, one account and one upstream model id.
//! Within a provider the endpoint speaking the client's style comes first (research R27),
//! then the model's `wires` in declared order, then the provider's other endpoints for
//! the type in declared order.

use nullrouter_registry::schema::{Endpoint, ModelType, ProviderEntity};
use nullrouter_registry::{NotFound, Registry, Resolution};

use crate::accounts::{self, Account, Accounts};
use crate::attempt::Pin;
use crate::models_live::LiveModels;
use crate::records::ErrorClass;
use crate::tokens::TokenCells;
use crate::verdict::{self, Verdicts};

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
    /// A combo has no plan of its own: each unified model it walks is planned in its turn.
    #[error("{name} is a combo")]
    Combo { name: String },
}

impl PlanError {
    /// The HTTP status the client sees.
    pub fn status(&self) -> u16 {
        match self {
            Self::NotFound(_) | Self::NoEndpoint { .. } => 404,
            Self::NoAccount { .. } => 503,
            Self::TypeMismatch { .. } | Self::Combo { .. } => 400,
        }
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

/// The live state a plan reads besides the registry and the accounts.
#[derive(Clone, Copy)]
pub struct Live<'a> {
    pub tokens: &'a TokenCells,
    pub live: &'a LiveModels,
    /// BROKEN pairs become skips (spec 011, research R11).
    pub verdicts: &'a Verdicts,
    /// A model test's one account: every other step is left out, and a BROKEN verdict doesn't
    /// skip it, since the test is what can settle it again.
    pub pin: Option<&'a Pin>,
}

/// What every member of one request is planned for.
#[derive(Clone, Copy)]
struct Ask<'a> {
    ty: ModelType,
    client_style: &'a str,
    tokens: &'a TokenCells,
    live: &'a LiveModels,
    verdicts: &'a Verdicts,
    pinned: bool,
}

/// The skip for a pair a test or the operator found BROKEN.
fn broken_skip(verdicts: &Verdicts, provider: &str, account: &str, model: &str) -> Option<Skip> {
    let v = verdicts.get(provider, account, model).filter(|v| v.state == verdict::State::Broken)?;
    Some(Skip {
        provider: provider.to_owned(),
        account: (account != verdict::NO_ACCOUNT).then(|| account.to_owned()),
        model: model.to_owned(),
        reason: format!("BROKEN since {}: {}", crate::clock::rfc3339(v.at), v.reason),
        class: Some(ErrorClass::Broken),
    })
}

/// One member's steps: its enabled accounts in operator order, or one skip.
/// A model the static catalog lacks but the provider's live list holds is served with the
/// list's type; one in neither list is not found unless uncatalogued models are allowed.
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
    let Ask { ty, client_style, tokens, live, verdicts, pinned } = *ask;
    let model = registry.model(&provider.id, requested).ok().and_then(|m| m.model);
    if model.is_none() && provider.models_live.is_some() {
        match live.find(&provider.id, requested) {
            Some(m) if m.ty != ty => {
                return Err(PlanError::TypeMismatch {
                    target: format!("{}/{requested}", provider.id),
                    model: m.ty,
                    route: ty,
                });
            }
            Some(_) => {}
            None if !provider.passthrough_models && !registry.settings(&provider.id).allow_uncatalogued_models => {
                return Err(NotFound::Model { provider: provider.id.clone(), model: requested.to_owned() }.into());
            }
            None => {}
        }
    }
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
    let broken = |account: &str| match pinned {
        true => None,
        false => broken_skip(verdicts, &provider.id, account, &upstream_id).map(Step::Skip),
    };
    if provider.auth.as_ref().is_some_and(|a| a.no_auth) {
        return Ok(vec![broken(verdict::NO_ACCOUNT).unwrap_or_else(|| candidate(None))]);
    }
    let mine: Vec<&Account> = accounts.for_provider(&provider.id).collect();
    if mine.is_empty() {
        return Err(PlanError::NoAccount { provider: provider.id.clone() });
    }
    Ok(mine
        .into_iter()
        .map(|a| match accounts::out_of_service(a, tokens) {
            None => broken(&a.name).unwrap_or_else(|| candidate(Some(a))),
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

/// The candidates behind one request's target (spec 006, R4): for each member of a unified
/// target, in declared order, its enabled accounts in operator order. A member that isn't
/// installed, has no endpoint for the type or has no account is a skip, and so is an account
/// that is out of service. Which one is tried first is the placement's decision
/// (`routing::place`), not this list's. `live` holds the providers' live model lists, which
/// resolve beside the static catalog. A BROKEN pair is a skip of class `broken`; with `pin`
/// set, only the pinned account's steps are kept.
pub fn plan<'s>(
    registry: &'s Registry,
    accounts: &'s Accounts,
    state: Live<'_>,
    target: &'s str,
    ty: ModelType,
    client_style: &str,
) -> Result<RequestPlan<'s>, PlanError> {
    let mut plan = plan_all(registry, accounts, state, target, ty, client_style)?;
    if let Some(pin) = state.pin {
        plan.steps.retain(|s| {
            let (provider, account) = match s {
                Step::Try(c) => (c.provider.id.as_str(), c.account.map_or(verdict::NO_ACCOUNT, |a| a.name.as_str())),
                Step::Skip(k) => (k.provider.as_str(), k.account.as_deref().unwrap_or(verdict::NO_ACCOUNT)),
            };
            provider == pin.provider && account == pin.account
        });
    }
    Ok(plan)
}

fn plan_all<'s>(
    registry: &'s Registry,
    accounts: &'s Accounts,
    state: Live<'_>,
    target: &'s str,
    ty: ModelType,
    client_style: &str,
) -> Result<RequestPlan<'s>, PlanError> {
    let Live { tokens, live, verdicts, pin } = state;
    let resolution = registry.resolve_with(target, |p, m| live.has(p, m))?;
    let declared = match &resolution {
        Resolution::Direct { provider, requested, .. } => {
            registry.model(&provider.id, requested).ok().and_then(|m| m.kind)
        }
        Resolution::Unified(u) => u.kind,
        Resolution::Combo(c) => return Err(PlanError::Combo { name: c.name.clone() }),
    };
    // An undeclared kind passes: the endpoint list decides.
    if let Some(model) = declared.and_then(ModelType::from_capability)
        && model != ty
    {
        return Err(PlanError::TypeMismatch { target: target.to_owned(), model, route: ty });
    }
    let ask = Ask { ty, client_style, tokens, live, verdicts, pinned: pin.is_some() };
    match resolution {
        Resolution::Direct { provider, requested, upstream_id, .. } => {
            let steps = member(registry, accounts, provider, requested, upstream_id, &ask)?;
            Ok(RequestPlan { steps, unified: None })
        }
        Resolution::Unified(u) => {
            let mut steps = Vec::new();
            for m in &u.members {
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
        Resolution::Combo(c) => Err(PlanError::Combo { name: c.name.clone() }),
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
