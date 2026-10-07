pub(crate) mod accounts;
pub(crate) mod behaviour;
pub(crate) mod check;
pub(crate) mod combos;
pub(crate) mod dashboard;
pub(crate) mod keys;
pub(crate) mod model;
pub(crate) mod plugins;
pub(crate) mod providers;
pub(crate) mod quota;
pub(crate) mod records;
pub(crate) mod resolve;
pub(crate) mod routing;
pub(crate) mod serve;
pub(crate) mod test;
pub(crate) mod unified;
pub(crate) mod validate;
pub(crate) mod verdicts;

use std::process::ExitCode;

use nullrouter_registry::OperatorHome;
use nullrouter_server::views::{self, Live, View, ViewError};
use serde_json::Value;

pub(crate) use nullrouter_cli::apply;

/// A view's builder: the home, the command's arguments, and the live answers.
pub(crate) type Build = fn(&OperatorHome, &Value, &Live) -> Result<View, ViewError>;

/// A read by the CLI route: fetches the view's live answers over the operator socket, builds
/// it, and turns its error into what the CLI prints and the exit code it gives.
pub(crate) fn read(home: &OperatorHome, needs: &[&'static str], args: &Value, build: Build) -> Result<View, ExitCode> {
    let live = views::fetch_socket(home, needs, args);
    build(home, args, &live).map_err(|e| {
        eprintln!("{}", e.message);
        ExitCode::from(e.code)
    })
}
