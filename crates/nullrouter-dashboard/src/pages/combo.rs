//! Combo (research R15): combos are not built yet. The page says what exists instead; the
//! frame lists the `combo` notices (dropped unified models, limits notes) above it.

use maud::html;
use serde_json::json;

use super::{Body, Ctx, Failure, Req};
use crate::components::{card, empty};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[ViewName::Check];

pub const TITLE: &str = "Combos are not built yet.";
pub const HINT: &str =
    "Declare unified models today with `[[unified_model]]` in config.toml and list them with `nullrouter unified`.";

pub fn wants(_req: &Req) -> Vec<Want> {
    vec![Want::new(ViewName::Check, json!({}))]
}

pub fn body(_ctx: &Ctx<'_>) -> Result<Body, Failure> {
    Ok(Body::new(card(None, html! { (empty("layers", TITLE, HINT)) })))
}
