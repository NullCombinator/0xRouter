//! Turning a test call's failure into BROKEN or UNKNOWN (spec 011 research R3). Used only for
//! tests: client traffic keeps `classify` unchanged (FR-010).

use nullrouter_registry::ProviderEntity;

use super::{Rejection, State};
use crate::records::ErrorClass;

/// The most characters a verdict's reason keeps.
pub const MAX_REASON_CHARS: usize = 500;

/// One failed attempt, as its record has it. `message` is already redacted.
#[derive(Debug, Clone, Copy)]
pub struct Failed<'a> {
    pub status: Option<u16>,
    pub class: ErrorClass,
    pub message: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judged {
    pub state: State,
    pub rejection: Option<Rejection>,
    pub reason: String,
}

/// Statuses the core list reads as a possible rejection of the model.
const REJECTING: [u16; 5] = [400, 403, 404, 405, 422];

/// The core list, in the order the rows are tried: each reason with its phrases. The message must
/// also contain `model`.
const CORE: [(Rejection, &[&str]); 3] = [
    (
        Rejection::ModelNotFound,
        &[
            "not found",
            "does not exist",
            "not_found",
            "unknown model",
            "no such model",
            "invalid model",
            "not a valid model",
        ],
    ),
    (
        Rejection::ModelNotAvailable,
        &["access to", "not available", "not allowed", "not enabled", "permission", "not entitled", "your plan", "tier"],
    ),
    (
        Rejection::TypeNotSupported,
        &[
            "does not support",
            "not supported",
            "unsupported",
            "only supports",
            "is not a chat model",
            "not a text model",
        ],
    ),
];

/// Classes that are about the account, not the model (FR-009).
fn account_class(class: ErrorClass) -> bool {
    matches!(class, ErrorClass::Refused | ErrorClass::NeedsSignIn | ErrorClass::TokenRefreshing | ErrorClass::NoAccount)
}

/// A rejected key or token, as `attempt::token_rejected` reads it, or a `[[signin.refused]]` match.
pub fn auth_rejection(provider: &ProviderEntity, status: u16, message: &str) -> bool {
    if provider.signin.as_ref().is_some_and(|d| d.refuses(status, message)) {
        return true;
    }
    match status {
        401 => true,
        403 => {
            let m = message.to_lowercase();
            crate::attempt::AUTH_REJECTION.iter().any(|t| m.contains(t))
        }
        _ => false,
    }
}

/// Whether `f` is about the account rather than the model: the test leaves the verdict as it
/// was (FR-009).
pub fn account_fault(f: &Failed<'_>, provider: &ProviderEntity) -> bool {
    account_class(f.class)
        || f.class == ErrorClass::Auth
        || f.status.is_some_and(|s| auth_rejection(provider, s, f.message))
}

/// What `f` says about the model. Order (R3): not definitive first (no status, 408, 429, 5xx, an
/// account rejection), then the plugin's `[[rejections]]`, then the core list, else UNKNOWN.
pub fn judge(f: &Failed<'_>, provider: &ProviderEntity) -> Judged {
    let unknown = || Judged { state: State::Unknown, rejection: None, reason: reason(f) };
    let Some(status) = f.status else { return unknown() };
    if matches!(status, 408 | 429) || !(400..=499).contains(&status) || account_class(f.class) {
        return unknown();
    }
    if auth_rejection(provider, status, f.message) {
        return unknown();
    }
    let broken = |r: Rejection| Judged { state: State::Broken, rejection: Some(r), reason: reason(f) };
    if let Some(rule) = provider.rejections.iter().find(|r| r.matches(status, f.message)) {
        return broken(Rejection::Plugin(rule.reason.as_str().to_owned()));
    }
    if !REJECTING.contains(&status) {
        return unknown();
    }
    let m = f.message.to_lowercase();
    if !m.contains("model") {
        return unknown();
    }
    match CORE.iter().find(|(_, phrases)| phrases.iter().any(|p| m.contains(p))) {
        Some((r, _)) => broken(r.clone()),
        None => unknown(),
    }
}

/// `<status>: <message>`, or the message alone without a status; at most [`MAX_REASON_CHARS`].
pub fn reason(f: &Failed<'_>) -> String {
    let text = match f.status {
        Some(s) => format!("{s}: {}", f.message),
        None => f.message.to_owned(),
    };
    cut(&text)
}

/// `text` cut to [`MAX_REASON_CHARS`] characters.
pub fn cut(text: &str) -> String {
    text.chars().take(MAX_REASON_CHARS).collect()
}
