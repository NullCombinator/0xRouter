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
