//! Console Log (research R15): not built yet; request records are on the Usage page.

use maud::html;

use super::{Body, Ctx, Failure, Req};
use crate::components::{card, empty, link_button};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[];

pub const TITLE: &str = "The console log is not built yet.";
pub const HINT: &str = "Request records are on the Usage page.";

pub fn wants(_req: &Req) -> Vec<Want> {
    Vec::new()
}

pub fn body(_ctx: &Ctx<'_>) -> Result<Body, Failure> {
    Ok(Body::new(card(
        None,
        html! { (empty("terminal", TITLE, HINT)) div class="empty__action" { (link_button("/usage", "Open Usage", Some("bar_chart"), true)) } },
    )))
}
