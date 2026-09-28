//! Real client SDKs and harnesses (T058) against the server over a scripted provider:
//! runs `tests/harness/run.sh`. Skipped unless `ZR_HARNESS=1` (needs the SDKs from
//! `tests/harness/README.md`).

mod common;

use std::path::Path;

use common::{chat_stream, chat_whole_text, server};
use serde_json::Value;

#[tokio::test(flavor = "multi_thread")]
async fn sdks_and_harnesses_accept_every_style() {
    if std::env::var("ZR_HARNESS").as_deref() != Ok("1") {
        eprintln!("skipped: set ZR_HARNESS=1 to run the harness scripts");
        return;
    }
    let s = server().await;
    // Real clients send as many requests as they like, in their own order.
    s.mock.respond(|r| if r.json()["stream"] == Value::Bool(true) { chat_stream() } else { chat_whole_text("Hello") });
    let run = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/harness/run.sh");
    let (base, key) = (s.base.clone(), s.key.clone());
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new("bash")
            .arg(run)
            .env("ZR_BASE", base)
            .env("ZR_KEY", key)
            .env("ZR_MODEL", "mockco/m1")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{text}");
    assert!(out.status.success(), "{text}");
}
