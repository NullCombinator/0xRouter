//! Spec 013 US3: connection settings in `config.toml`.

use std::sync::Arc;

use nullrouter_registry::{OperatorHome, Registry, RegistryHandle, StartupError};
use tempfile::TempDir;

fn open(config: &str) -> (TempDir, Result<Arc<Registry>, StartupError>) {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), config).unwrap();
    let reg = RegistryHandle::open_parity(OperatorHome::new(home.path())).map(|h| h.snapshot());
    (home, reg)
}

fn ok(config: &str) -> Arc<Registry> {
    open(config).1.unwrap_or_else(|e| panic!("{e}"))
}

fn errors(config: &str) -> String {
    match open(config).1 {
        Ok(_) => panic!("config was accepted:\n{config}"),
        Err(e) => e.to_string(),
    }
}

const CONFIG: &str = r#"
[connection]
proxy = "eu-exit"

[provider.openrouter.connection]
connect_timeout_ms = 5000
header_timeout_ms = 10000
first_token_timeout_ms = 60000
stall_timeout_ms = 360000
reuse = true
http2 = false
proxy = "none"

[provider.openrouter.model."anthropic/claude-opus-4.1".connection]
first_token_timeout_ms = 300000
"#;

#[test]
fn provider_and_model_connection_settings_parse() {
    let reg = ok(CONFIG);
    let s = reg.settings("openrouter");
    let c = &s.connection;
    assert_eq!(
        (c.connect_timeout_ms, c.header_timeout_ms, c.first_token_timeout_ms, c.stall_timeout_ms),
        (Some(5000), Some(10_000), Some(60_000), Some(360_000))
    );
    assert_eq!((c.reuse, c.http2, c.proxy.as_deref()), (Some(true), Some(false), Some("none")));
    let m = &s.model["anthropic/claude-opus-4.1"].connection;
    assert_eq!((m.first_token_timeout_ms, m.connect_timeout_ms), (Some(300_000), None));
    assert_eq!(reg.settings("anthropic").connection, Default::default());
}

#[test]
fn a_first_token_timeout_may_be_off_and_the_others_may_not() {
    let reg = ok("[provider.openrouter.connection]\nfirst_token_timeout_ms = 0\n");
    assert_eq!(reg.settings("openrouter").connection.first_token_timeout_ms, Some(0));

    for key in ["connect_timeout_ms", "header_timeout_ms", "stall_timeout_ms"] {
        let e = errors(&format!("[provider.openrouter.connection]\n{key} = 0\n"));
        assert!(e.contains(key) && e.contains("out of range"), "{e}");
    }
    let e = errors("[provider.openrouter.connection]\nfirst_token_timeout_ms = 3600001\n");
    assert!(e.contains("first_token_timeout_ms") && e.contains("0 for off"), "{e}");
    let e = errors("[provider.openrouter.model.\"m\".connection]\nheader_timeout_ms = 4000000\n");
    assert!(e.contains("header_timeout_ms") && e.contains("out of range"), "{e}");
}

#[test]
fn a_model_takes_timeouts_only() {
    let e = errors("[provider.openrouter.model.\"m\".connection]\nproxy = \"x\"\n");
    assert!(e.contains("proxy"), "{e}");
}

#[test]
fn reuse_and_http2_are_read_per_provider() {
    let reg = ok("[provider.openrouter.connection]\nreuse = false\nhttp2 = false\n");
    let c = reg.settings("openrouter").connection;
    assert_eq!((c.reuse, c.http2), (Some(false), Some(false)));
    let c = reg.settings("anthropic").connection;
    assert_eq!((c.reuse, c.http2), (None, None));
}

#[test]
fn retry_settings_are_read_and_capped() {
    let reg = ok("[provider.openrouter.retry]\nall = { retries = 2, delay_ms = 1000 }\n\"503\" = { retries = 3, delay_ms = 2000 }\n");
    let r = reg.settings("openrouter").retry;
    assert_eq!(r.all.map(|o| (o.retries, o.delay_ms)), Some((2, 1000)));
    assert_eq!(r.by_status["503"].retries, 3);

    let e = errors("[provider.openrouter.retry]\nall = { retries = 6 }\n");
    assert!(e.contains("retries") && e.contains("0-5"), "{e}");
    let e = errors("[provider.openrouter.retry]\n\"503\" = { retries = 1, delay_ms = 30001 }\n");
    assert!(e.contains("0-30000"), "{e}");
    let e = errors("[provider.openrouter.retry]\n\"50\" = { retries = 1 }\n");
    assert!(e.contains("3-digit"), "{e}");
}
