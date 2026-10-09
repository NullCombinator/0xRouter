//! Test fixture: the smallest adapter. It does nothing to any request.

use nullrouter_adapter_kit::{Adapter, Context, Edits, Input};

pub struct Noop;

impl Adapter for Noop {
    fn on_request(_ctx: &Context, _input: &Input, _out: &mut Edits) {}
}

nullrouter_adapter_kit::export!(Noop);
