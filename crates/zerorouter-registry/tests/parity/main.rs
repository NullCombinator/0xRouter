//! Parity against the 9router oracle in `tests/fixtures/9router/` (research R9).
//! On a difference, fix the generator or the views, never the fixtures.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use zerorouter_registry::{OperatorHome, Registry, RegistryHandle};

mod alias;
mod deviations;
mod lookup;
mod oauth;
mod transport;

/// Model types a chosen provider's schema 2 file doesn't carry, with why: (provider,
/// type, reason). Oracle rows of these are skipped, and a skip that finds the type carried
/// fails, so the list can't go stale.
pub(crate) const NOT_CARRIED: &[(&str, &str, &str)] = &[
    ("opencode-zen", "systemone", "systemone sections are unsupported (fit check)"),
    ("openrouter", "systemone", "systemone sections are unsupported (fit check)"),
];

pub(crate) fn not_carried(provider: &str, ty: &str) -> bool {
    NOT_CARRIED.iter().any(|(p, t, _)| *p == provider && *t == ty)
}

/// The `data` member of `tests/fixtures/9router/<name>.json`.
pub(crate) fn fixture(name: &str) -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/9router").join(format!("{name}.json"));
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut doc: Value = serde_json::from_str(&src).unwrap();
    doc["data"].take()
}

/// A registry with bundled plugins only.
pub(crate) fn bundled() -> Arc<Registry> {
    let home = tempfile::tempdir().unwrap();
    RegistryHandle::open(OperatorHome::new(home.path())).unwrap().snapshot()
}
