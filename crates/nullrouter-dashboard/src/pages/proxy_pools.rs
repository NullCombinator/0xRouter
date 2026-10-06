//! Proxy Pools (research R15): not built; 0router has no proxy pool feature today.

use maud::html;

use super::{Body, Ctx, Failure, Req};
use crate::components::{card, empty};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[];

pub const TITLE: &str = "Proxy pools are not built yet.";
pub const HINT: &str = "0router has no proxy pool feature today.";

pub fn wants(_req: &Req) -> Vec<Want> {
    Vec::new()
}

pub fn body(_ctx: &Ctx<'_>) -> Result<Body, Failure> {
    Ok(Body::new(card(None, html! { (empty("lan", TITLE, HINT)) })))
}
