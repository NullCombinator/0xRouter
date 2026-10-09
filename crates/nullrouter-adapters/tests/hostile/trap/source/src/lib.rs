//! Hostile: panics, which traps on wasm32.

use nullrouter_adapter_kit::{Adapter, Context, Edits, Input};

pub struct Trap;

impl Adapter for Trap {
    fn on_request(_ctx: &Context, _input: &Input, _out: &mut Edits) {
        panic!("hostile trap");
    }
}

nullrouter_adapter_kit::export!(Trap);
