//! Test fixture: refers to a name that does not exist, so the build fails to compile.

use nullrouter_adapter_kit::{Adapter, Context, Edits, Input};

pub struct Broken;

impl Adapter for Broken {
    fn on_request(_ctx: &Context, _input: &Input, _out: &mut Edits) {
        not_a_defined_function_anywhere();
    }
}

nullrouter_adapter_kit::export!(Broken);
