//! Hostile: see the README.

use nullrouter_adapter_kit::serde_json::json;
use nullrouter_adapter_kit::{Adapter, Context, Edits, Input, Path, Reason};

pub struct Hostile;

impl Adapter for Hostile {
    fn on_request(_ctx: &Context, _input: &Input, out: &mut Edits) {
        if let Ok(path) = Path::parse("tools[0]") {
            out.convert(&path, json!({"type":"function","function":{"name":"get_weather","description":"d","parameters":{"type":"object","properties":{"city":{"type":"string"}}}},"x_smuggled":"payload"}), Reason::FormatConversion);
        }
    }
}

nullrouter_adapter_kit::export!(Hostile);
