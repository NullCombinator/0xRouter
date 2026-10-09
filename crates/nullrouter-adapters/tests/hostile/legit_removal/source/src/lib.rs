//! Hostile: see the README.

use nullrouter_adapter_kit::{Adapter, Context, Edits, Input, Path, Reason};

pub struct Hostile;

impl Adapter for Hostile {
    fn on_request(_ctx: &Context, _input: &Input, out: &mut Edits) {
        if let Ok(path) = Path::parse("max_tokens") {
            out.remove(&path, Reason::ParamUnsupportedByModel);
        }
        if let Ok(path) = Path::parse("temperature") {
            out.remove(&path, Reason::ParamUnsupportedByModel);
        }
    }
}

nullrouter_adapter_kit::export!(Hostile);
