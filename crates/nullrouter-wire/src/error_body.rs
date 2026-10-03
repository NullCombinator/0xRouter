//! Informational error bodies in each style's error shape (research R11).
//!
//! Bindings: `error.type` (the style's type for the status), `error.message`,
//! `error.details` (the `nullrouter` extra object), and `error.status` / `error.code`
//! (both the HTTP status number).

use serde::Serialize;
use serde_json::{Value, json};

use crate::codec::Style;
use crate::template::{Bindings, render};

/// `style`'s error body for `status`.
pub fn body(style: &Style, status: u16, message: &str, details: Value) -> Value {
    let b = Bindings::new()
        .with("error.type", style.error_type(status))
        .with("error.message", message)
        .with("error.details", details)
        .with("error.status", status)
        .with("error.code", status);
    render(&style.error_body, &b)
}

/// One tried target, for the message and the `nullrouter` details: same-account retries
/// are folded into `retries`, and a skipped target has no status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tried {
    pub provider: String,
    pub account: Option<String>,
    pub model: String,
    pub status: Option<u16>,
    pub class: Option<String>,
    pub reason: String,
    pub retries: u32,
}

/// The command that brings an out-of-service sign-in account back.
const SIGNIN_COMMAND: &str = "run nullrouter accounts signin";

impl Tried {
    /// `provider/account model: [status ]reason[ after N retries]`. An out-of-service
    /// sign-in account (class `needs_sign_in` or `refused`) reads as
    /// `provider/account: needs sign-in — run nullrouter accounts signin provider account`
    /// (research R10, FR-016).
    pub fn line(&self) -> String {
        let who = match &self.account {
            Some(a) => format!("{}/{a}", self.provider),
            None => self.provider.clone(),
        };
        if let (Some(a), Some(class)) = (&self.account, self.class.as_deref()) {
            let state = match class {
                "needs_sign_in" => Some("needs sign-in".to_owned()),
                // `refused by provider: <reason>; run …` → `refused by provider (<reason>)`.
                "refused" => Some(match self.reason.split_once("; run ").map(|(r, _)| r) {
                    Some(r) => match r.split_once(": ") {
                        Some((head, why)) => format!("{head} ({why})"),
                        None => r.to_owned(),
                    },
                    None => "refused by provider".to_owned(),
                }),
                _ => None,
            };
            if let Some(state) = state {
                return format!("{who}: {state} — {SIGNIN_COMMAND} {} {a}", self.provider);
            }
        }
        let status = self.status.map(|s| format!("{s} ")).unwrap_or_default();
        let retries = match self.retries {
            0 => String::new(),
            1 => " after 1 retry".to_owned(),
            n => format!(" after {n} retries"),
        };
        format!("{who} {}: {status}{}{retries}", self.model, self.reason)
    }
}

/// The informational message (research R11): the summary line with the record id, then
/// one line per tried target.
pub fn message(summary: &str, record_id: &str, tried: &[Tried]) -> String {
    let mut out = format!("{summary} (record {record_id})");
    for t in tried {
        out.push('\n');
        out.push_str(&t.line());
    }
    out
}

/// The `nullrouter` extra object.
pub fn details(record_id: &str, tried: &[Tried]) -> Value {
    json!({ "record_id": record_id, "attempts": tried })
}

/// The OpenAI error shape, for requests that matched no style (unknown paths).
pub fn openai(status: u16, kind: &str, message: &str) -> Value {
    serde_json::json!({ "error": { "message": message, "type": kind, "code": status } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_in_the_style_shape() {
        let f = nullrouter_registry::validate::validate_style(
            include_str!("../tests/fixtures/mini-style.toml"),
            "mini-style.toml",
        )
        .unwrap();
        let s = Style::compile(&f).unwrap();
        let v = body(&s, 401, "0router: unknown access key", json!({ "record_id": "rq_1" }));
        assert_eq!(
            v,
            json!({
                "type": "error",
                "error": { "type": "authentication_error", "message": "0router: unknown access key" },
                "nullrouter": { "record_id": "rq_1" }
            })
        );
    }

    #[test]
    fn the_message_names_every_tried_target() {
        let t = |account: Option<&str>, status, reason: &str, retries| Tried {
            provider: "acme".into(),
            account: account.map(Into::into),
            model: "m1".into(),
            status,
            class: None,
            reason: reason.into(),
            retries,
        };
        let tried = [
            t(Some("main"), Some(503), "overloaded", 3),
            t(Some("backup"), None, "cooling down for 4 s", 0),
            t(None, Some(502), "network error", 1),
        ];
        assert_eq!(
            message("0router: no provider could serve m1", "rq_1", &tried),
            "0router: no provider could serve m1 (record rq_1)\nacme/main m1: 503 overloaded after 3 retries\nacme/backup m1: cooling down for 4 s\nacme m1: 502 network error after 1 retry"
        );
        assert_eq!(details("rq_1", &tried[..1])["attempts"][0]["retries"], 3);
    }

    #[test]
    fn out_of_service_accounts_name_the_command() {
        let t = |class: &str, reason: &str, status| Tried {
            provider: "xai".into(),
            account: Some("main".into()),
            model: "grok-4".into(),
            status,
            class: Some(class.into()),
            reason: reason.into(),
            retries: 0,
        };
        assert_eq!(
            t("needs_sign_in", "needs sign-in: run nullrouter accounts signin xai main", None).line(),
            "xai/main: needs sign-in — run nullrouter accounts signin xai main"
        );
        assert_eq!(
            t(
                "refused",
                "refused by provider: only for Claude Code; run nullrouter accounts signin xai main",
                Some(403)
            )
            .line(),
            "xai/main: refused by provider (only for Claude Code) — run nullrouter accounts signin xai main"
        );
        assert_eq!(
            t("token_refreshing", "token expired, refresh retrying", None).line(),
            "xai/main grok-4: token expired, refresh retrying"
        );
    }
}
