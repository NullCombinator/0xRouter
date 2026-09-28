//! Informational error bodies in each style's error shape (research R11).
//!
//! Bindings: `error.type` (the style's type for the status), `error.message`,
//! `error.details` (the `zerorouter` extra object), and `error.status` / `error.code`
//! (both the HTTP status number).

use serde_json::Value;

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

/// The OpenAI error shape, for requests that matched no style (unknown paths).
pub fn openai(status: u16, kind: &str, message: &str) -> Value {
    serde_json::json!({ "error": { "message": message, "type": kind, "code": status } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_in_the_style_shape() {
        let f = zerorouter_registry::validate::validate_style(include_str!("../tests/fixtures/mini-style.toml"), "mini-style.toml").unwrap();
        let s = Style::compile(&f).unwrap();
        let v = body(&s, 401, "0router: unknown access key", json!({ "record_id": "rq_1" }));
        assert_eq!(
            v,
            json!({
                "type": "error",
                "error": { "type": "authentication_error", "message": "0router: unknown access key" },
                "zerorouter": { "record_id": "rq_1" }
            })
        );
    }
}
