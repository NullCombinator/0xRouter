//! hermes, the built-in adapter. Its rules arrive with user story 1; until then it makes no
//! edits, so a key bound to it runs as a plain client.

use nullrouter_adapter_kit::{Context, Edits};
use serde_json::Value;

use crate::selector::Selector;

pub const NAME: &str = "hermes";

/// hermes reads the whole request body.
pub fn request_selectors() -> Vec<Selector> {
    vec![Selector::parse("$").expect("`$` is a valid selector")]
}

pub fn on_request(_ctx: &Context, _body: &Value) -> Edits {
    Edits::default()
}
