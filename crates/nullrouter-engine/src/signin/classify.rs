//! Refresh-failure classification: permanent (needs sign-in) or transient (research R10).
//!
//! | Refresh outcome | Class |
//! |---|---|
//! | `invalid_grant`, `invalid_request`, `unauthorized_client`, `refresh_token_expired`, `refresh_token_reused`, `refresh_token_invalidated` | permanent |
//! | any other 400/401/403 from the token endpoint | permanent |
//! | timeout, connection failure, 5xx, 429 | transient |
//!
//! Outside the table: a token endpoint answering some other status (404, 405, …) or a 2xx
//! without an access token is transient, so an outage that misroutes the endpoint never
//! takes an account out of service. Nothing in a sign-in declaration can be fixed by
//! signing in again either, so those are transient too.

use super::SignInError;

/// The OAuth error codes that always mean the refresh token is dead (research R10, R19:
/// 9router retries `refresh_token_reused` and `unauthorized_client`; 0router doesn't).
pub const PERMANENT_CODES: [&str; 6] = [
    "invalid_grant",
    "invalid_request",
    "unauthorized_client",
    "refresh_token_expired",
    "refresh_token_reused",
    "refresh_token_invalidated",
];

/// What a failed refresh means for the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshClass {
    /// The refresh token can't recover: the account needs signing in again.
    Permanent,
    /// Retry with backoff; the account stays in service while its token is valid.
    Transient,
}

/// Research R10's class of a refresh failure.
pub fn classify(e: &SignInError) -> RefreshClass {
    match e {
        SignInError::Rejected { status, code, .. } => {
            if PERMANENT_CODES.contains(&code.as_str()) || matches!(status, 400 | 401 | 403) {
                RefreshClass::Permanent
            } else {
                RefreshClass::Transient
            }
        }
        _ => RefreshClass::Transient,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(status: u16, code: &str) -> SignInError {
        SignInError::Rejected { what: "token endpoint", status, code: code.into() }
    }

    #[test]
    fn r10_table() {
        for code in PERMANENT_CODES {
            assert_eq!(classify(&rejected(400, code)), RefreshClass::Permanent, "{code}");
            assert_eq!(classify(&rejected(500, code)), RefreshClass::Permanent, "{code}: the code wins");
        }
        for s in [400, 401, 403] {
            assert_eq!(classify(&rejected(s, "HTTP 4xx")), RefreshClass::Permanent, "{s}");
        }
        for s in [429, 500, 502, 503, 404] {
            assert_eq!(classify(&rejected(s, "server_error")), RefreshClass::Transient, "{s}");
        }
        let t = SignInError::Transport { what: "token endpoint", reason: "timed out".into() };
        assert_eq!(classify(&t), RefreshClass::Transient);
        let b = SignInError::BadResponse { what: "token endpoint", reason: "not JSON".into() };
        assert_eq!(classify(&b), RefreshClass::Transient);
    }
}
