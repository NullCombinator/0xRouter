//! Spec 014 T033: a provider the operator disabled (`[provider.<id>] enabled = false`) is
//! skipped by routing without a call, and one that is enabled is served as before.

mod common;

use common::*;

fn plugin(mock: &nullrouter_engine::testkit::MockUpstream) -> Vec<(&'static str, String)> {
    let toml = format!(
        "schema = 2\nid = \"vis\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n",
        mock.url("/vis/chat/completions")
    );
    vec![("vis", toml)]
}

#[tokio::test]
async fn routing_skips_a_disabled_provider_and_serves_an_enabled_one() {
    let on = setup(plugin, &[("vis", "main")], "").await;
    on.mock.on("/vis/chat", [ok()]);
    assert!(send(&on, "vis/m1").await.1.is_ok());

    let off = setup(plugin, &[("vis", "main")], "[provider.vis]\nenabled = false\n").await;
    off.mock.on("/vis/chat", [ok()]);
    assert!(send(&off, "vis/m1").await.1.is_err());
    assert!(off.mock.received().is_empty(), "a disabled provider is never called");
}
