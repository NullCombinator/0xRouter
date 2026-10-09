//! Hostile: asks for 128 MiB.

use nullrouter_adapter_kit::{Adapter, Context, Edits, Input};

pub struct Bomb;

impl Adapter for Bomb {
    fn on_request(_ctx: &Context, _input: &Input, _out: &mut Edits) {
        let big = vec![1u8; 128 << 20];
        std::hint::black_box(&big);
    }
}

nullrouter_adapter_kit::export!(Bomb);
