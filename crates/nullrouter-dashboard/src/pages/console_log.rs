//! Console Log (spec 014 US5, T039): the changes made from the dashboard, newest first, read from the
//! change log the CLI's `dashboard log` also reads. The time is the CLI's local text, with the zone
//! beside it; the result words are the CLI's.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use maud::html;

use super::{Body, Ctx, Failure, Req};
use crate::components::{card, empty};
use crate::log::{self, Outcome};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[];

/// The newest changes the page shows, as the CLI's default.
const LIMIT: usize = 50;

pub fn wants(_req: &Req) -> Vec<Want> {
    Vec::new()
}

pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    let entries = log::read(ctx.engine.home(), LIMIT).map_err(|_| Failure {
        status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        message: "The change log can't be read.".to_owned(),
    })?;
    let content = if entries.is_empty() {
        card(None, empty("terminal", "No changes made from the dashboard yet.", ""))
    } else {
        card(
            None,
            html! {
                table {
                    thead { tr { th { "Time" } th { "Change" } th { "Subject" } th { "Result" } th { "Text" } } }
                    tbody {
                        @for e in &entries {
                            tr {
                                td { (ctx.time(&e.at)) @if let Some(z) = zone(&e.at, ctx.tz) { " " (z) } }
                                td { (e.op) }
                                td { (e.subject) }
                                td { (result_word(e.result)) }
                                td { (e.text) }
                            }
                        }
                    }
                }
            },
        )
    };
    Ok(Body::new(content))
}

/// The zone abbreviation of `at` in `tz` (`CEST`, or an offset for a fixed zone); `None` when `at`
/// isn't an RFC 3339 time.
fn zone(at: &str, tz: &TimeZone) -> Option<String> {
    let at = at.parse::<Timestamp>().ok()?;
    Some(at.to_zoned(tz.clone()).strftime("%Z").to_string())
}

/// The snake_case word a change's result prints as in the CLI.
fn result_word(result: Outcome) -> &'static str {
    match result {
        Outcome::Done => "done",
        Outcome::SavedNotApplied => "saved_not_applied",
        Outcome::Started => "started",
        Outcome::Refused => "refused",
    }
}
