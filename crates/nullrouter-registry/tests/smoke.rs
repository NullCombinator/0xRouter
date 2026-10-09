use nullrouter_registry::{OperatorHome, RegistryHandle};

#[test]
fn empty_home_loads_every_bundled_provider() {
    let home = tempfile::tempdir().unwrap();
    let handle = RegistryHandle::open_parity(OperatorHome::new(home.path())).unwrap();
    let reg = handle.snapshot();
    assert_eq!(reg.providers().count(), 121);
    let report = reg.report();
    assert_eq!((report.bundled, report.user, report.unified_models), (121, 0, 0));
    assert!(report.skipped.is_empty() && report.pending_conflicts.is_empty());
}

/// Slice 011: `[tests]` defaults when absent, and each rule reported at its position.
#[test]
fn test_settings_load_and_rules() {
    use std::time::Duration;

    let open = |config: &str| {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), config).unwrap();
        RegistryHandle::open(OperatorHome::new(home.path())).map(|h| h.snapshot())
    };
    let reg = open("schema = 1\n").unwrap();
    let t = &reg.runtime().tests;
    assert_eq!(t.retest, [60, 300, 1800, 21_600].map(Duration::from_secs));
    assert_eq!((t.broken_retest, t.concurrency), (None, 4));
    assert_eq!((t.timeout.text, t.timeout.image), (Duration::from_secs(30), Duration::from_secs(300)));

    let reg = open("[tests]\nretest = [\"2m\", \"2m\", \"1h\"]\nbroken_retest = \"12h\"\nconcurrency = 2\n").unwrap();
    assert_eq!(reg.runtime().tests.broken_retest, Some(Duration::from_secs(12 * 3600)));

    for (config, want) in [
        ("[tests]\nretest = [\"10s\"]\n", "config.toml:2:11 tests.retest[0]: at least 30s"),
        ("[tests]\nretest = [\"5m\", \"1m\"]\n", "tests.retest[1]: steps must not get shorter"),
        ("[tests]\nretest = []\n", "tests.retest: 1 to 10 steps"),
        (
            "[tests]\nretest = [\"1m\",\"1m\",\"1m\",\"1m\",\"1m\",\"1m\",\"1m\",\"1m\",\"1m\",\"1m\",\"1m\"]\n",
            "tests.retest: 1 to 10 steps",
        ),
        ("[tests]\nbroken_retest = \"30m\"\n", "tests.broken_retest: \"off\", \"on\" or at least 1h"),
        ("[tests]\nconcurrency = 0\n", "tests.concurrency: 1 to 32"),
        ("[tests]\nconcurrency = 33\n", "tests.concurrency: 1 to 32"),
        ("[tests.timeout]\ntext = \"1s\"\n", "tests.timeout.text: 5s to 30m"),
        ("[tests.timeout]\nvideo = \"31m\"\n", "tests.timeout.video: 5s to 30m"),
        ("[tests.timeout]\naudio = \"1m\"\n", "unknown type \"audio\""),
    ] {
        let err = open(config).err().unwrap_or_else(|| panic!("accepted: {config}")).to_string();
        assert!(err.contains(want), "{config}: {err}");
    }
}
