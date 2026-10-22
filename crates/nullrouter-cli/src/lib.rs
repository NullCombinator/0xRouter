//! The parts of the `nullrouter` CLI that drive an interactive flow, as a library so tests can
//! run them in-process against a mock identity provider: the shipped binary resolves the
//! provider from the operator's registry and calls the same code.

pub mod quota_text;
pub mod routing_text;
pub mod signin;

use nullrouter_registry::OperatorHome;
use nullrouter_server::ops::{self, Ctx};

/// After a file was saved: asks the running server to reload it. `Ok` carries what to
/// print (`applied`, or `saved; applies at next start` with no server); `Err` the reason
/// a running server refused the files, which then keeps its previous state. The reload's
/// notes (unified models whose members' limits differ) go to stderr.
pub fn apply(home: &OperatorHome) -> Result<&'static str, String> {
    let applied = ops::apply(&Ctx { home: home.clone(), engine: None })?;
    for n in &applied.notes {
        eprintln!("note: {n}");
    }
    Ok(applied.text)
}
