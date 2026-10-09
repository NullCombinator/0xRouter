//! Hostile: never returns.

use nullrouter_adapter_kit::{Adapter, Context, Edits, Input};

pub struct Spin;

impl Adapter for Spin {
    fn on_request(_ctx: &Context, _input: &Input, _out: &mut Edits) {
        loop {
            std::hint::spin_loop();
        }
    }
}

nullrouter_adapter_kit::export!(Spin);
