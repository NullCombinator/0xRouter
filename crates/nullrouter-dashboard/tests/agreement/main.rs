//! Each page against the CLI for the same moment (SC-001, brief row 9): every scalar the CLI's
//! `--json` holds for a fact the page shows appears on the page. One module per page.

#[path = "../common/mod.rs"]
mod common;

mod endpoint;
mod providers;
mod quota;
mod usage;
