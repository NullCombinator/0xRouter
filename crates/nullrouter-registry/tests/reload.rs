//! FR-024 – FR-026, SC-007: reload builds and validates a full candidate, then swaps it in
//! atomically; readers never see a mix.

use std::sync::atomic::{AtomicBool, Ordering};
use std::{fs, thread};

use nullrouter_registry::{OperatorHome, RegistryHandle, Resolution};

const ONE: &str = r#"
[[unified_model]]
name = "probe"
members = [{ provider = "kr", model = "claude-sonnet-4.5" }]
"#;

const TWO: &str = r#"
[[unified_model]]
name = "probe"
members = [
  { provider = "kr", model = "claude-sonnet-4.5" },
  { provider = "openrouter", model = "anthropic/claude-sonnet-4.5" },
]
"#;

fn members(handle: &RegistryHandle) -> usize {
    match handle.snapshot().resolve("probe") {
        Ok(Resolution::Unified(u)) => u.members.len(),
        other => panic!("probe resolved to {other:?}"),
    }
}

#[test]
fn readers_never_see_a_mix() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    fs::write(&config, ONE).unwrap();
    let handle = RegistryHandle::open_parity(OperatorHome::new(home.path())).unwrap();
    let done = AtomicBool::new(false);

    thread::scope(|s| {
        let readers: Vec<_> = (0..8)
            .map(|_| {
                s.spawn(|| {
                    let mut seen = [0usize; 3];
                    while !done.load(Ordering::Relaxed) {
                        let n = members(&handle);
                        assert!(n == 1 || n == 2, "{n} members");
                        seen[n] += 1;
                        thread::yield_now();
                    }
                    seen
                })
            })
            .collect();
        for i in 0..200 {
            fs::write(&config, if i % 2 == 0 { TWO } else { ONE }).unwrap();
            handle.reload().unwrap();
        }
        done.store(true, Ordering::Relaxed);
        for r in readers {
            let seen = r.join().unwrap();
            assert!(seen[1] + seen[2] > 0);
        }
    });
    assert_eq!(members(&handle), 1);
}

#[test]
fn invalid_reload_keeps_the_old_snapshot() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    fs::write(&config, ONE).unwrap();
    let handle = RegistryHandle::open_parity(OperatorHome::new(home.path())).unwrap();

    fs::write(
        &config,
        r#"
[[unified_model]]
name = "probe"
members = [{ provider = "xx", model = "m" }]
[provider.nope]
"#,
    )
    .unwrap();
    let err = handle.reload().unwrap_err();
    assert_eq!(err.errors.len(), 2, "{err}");
    let text = err.to_string();
    assert!(text.contains(r#"unknown provider "xx""#) && text.contains("provider.nope: unknown provider"), "{text}");
    assert_eq!(members(&handle), 1);
}

#[test]
fn no_change_reload_is_idempotent() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join("config.toml"), TWO).unwrap();
    let handle = RegistryHandle::open_parity(OperatorHome::new(home.path())).unwrap();
    let before = handle.snapshot();
    let report = handle.reload().unwrap();
    let after = handle.snapshot();
    assert_eq!(report.bundled, 121);
    assert_eq!(after.providers().count(), before.providers().count());
    let mut ids: Vec<_> = after.providers().map(|p| p.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 121);
    assert_eq!(after.unified_models().count(), 1);
}

#[test]
fn held_snapshot_is_unaffected_by_reload() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    fs::write(&config, ONE).unwrap();
    let handle = RegistryHandle::open_parity(OperatorHome::new(home.path())).unwrap();
    let held = handle.snapshot();
    fs::write(&config, TWO).unwrap();
    handle.reload().unwrap();
    assert_eq!(members(&handle), 2);
    let Ok(Resolution::Unified(u)) = held.resolve("probe") else { panic!() };
    assert_eq!(u.members.len(), 1);
}
