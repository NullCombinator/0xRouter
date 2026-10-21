//! Test fixture: an `unsafe` block inside `on_request`. `-F unsafe_code` must refuse it.

use nullrouter_adapter_kit::{Adapter, Context, Edits, Input};

pub struct UnsafeBlock;

impl Adapter for UnsafeBlock {
    fn on_request(_ctx: &Context, _input: &Input, _out: &mut Edits) {
        let _value = unsafe { std::ptr::read(&0u32) };
    }
}

nullrouter_adapter_kit::export!(UnsafeBlock);
