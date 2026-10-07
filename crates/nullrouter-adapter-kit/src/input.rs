//! The parts of the body that match an adapter's declared selectors.

use serde::{Deserialize, Serialize};

use crate::edit::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub path: Path,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub parts: Vec<Part>,
}

impl Input {
    pub fn parts(&self) -> impl Iterator<Item = (&Path, &serde_json::Value)> {
        self.parts.iter().map(|p| (&p.path, &p.value))
    }
}

/// What an adapter implements. `on_request` is required.
pub trait Adapter {
    fn on_request(ctx: &crate::context::Context, input: &Input, out: &mut crate::edit::Edits);

    fn on_response(_ctx: &crate::context::Context, _input: &Input, _out: &mut crate::edit::Edits) {}

    fn on_event(_ctx: &crate::context::Context, _input: &Input, _out: &mut crate::edit::Edits) {}
}
