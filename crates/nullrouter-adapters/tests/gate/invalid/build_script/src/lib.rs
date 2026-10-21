pub struct Noop;

impl nullrouter_adapter_kit::Adapter for Noop {
    fn on_request(
        _ctx: &nullrouter_adapter_kit::Context,
        _input: &nullrouter_adapter_kit::Input,
        _out: &mut nullrouter_adapter_kit::Edits,
    ) {}
}

nullrouter_adapter_kit::export!(Noop);
