use std::os::unix::fs::PermissionsExt;

use jiff::Timestamp;
use nullrouter_adapters::HarnessName;
use nullrouter_adapters::alerts::{AlertId, AlertKind, AlertLog, MAX_ALERTS, NewAlert};
use nullrouter_adapters::fingerprint;
use nullrouter_adapters::store::{Store, VersionId};

fn version(seed: &str) -> VersionId {
    let fp = fingerprint::of_files(&[("src/lib.rs".into(), seed.as_bytes().to_vec())]);
    VersionId::new(&"0.3.0".parse().unwrap(), &fp)
}

fn new(kind: AlertKind, v: &VersionId, codes: &'static [&'static str]) -> NewAlert {
    NewAlert {
        kind,
        harness: HarnessName::new("claude-code").unwrap(),
        version: v.clone(),
        record: Some("rq_01".into()),
        message: "the adapter failed",
        codes,
    }
}

fn at(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn open() -> (tempfile::TempDir, AlertLog) {
    let home = tempfile::tempdir().unwrap();
    let log = AlertLog::open(&Store::open(home.path()).unwrap());
    (home, log)
}

#[test]
fn an_alert_has_the_documented_fields_and_survives_a_reload() {
    let (home, log) = open();
    let v = version("a");
    let raised = log.raise(new(AlertKind::Guardrail, &v, &["tool_call_added"]), at("2026-10-08T10:00:00Z")).unwrap();

    assert!(AlertId::parse(raised.id.as_str()).is_some(), "{}", raised.id);
    assert_eq!(raised.detail, "the adapter failed: tool_call_added");
    assert_eq!((raised.count, raised.acked.as_deref()), (1, None));
    assert_eq!(raised.record.as_deref(), Some("rq_01"));

    let reopened = AlertLog::open(&Store::open(home.path()).unwrap());
    assert_eq!(reopened.list().unwrap(), vec![raised]);
    assert_eq!(reopened.unacked().unwrap(), 1);
}

#[test]
fn the_file_is_private_and_a_missing_one_is_an_empty_list() {
    let (home, log) = open();
    assert!(log.list().unwrap().is_empty());
    log.raise(new(AlertKind::Refused, &version("a"), &[]), at("2026-10-08T10:00:00Z")).unwrap();
    let mode = std::fs::metadata(home.path().join("adapters/alerts.toml")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn an_alert_id_is_al_and_ten_lowercase_characters() {
    for good in ["al_abcdefghij", "al_0123456789"] {
        assert!(AlertId::parse(good).is_some(), "{good}");
    }
    for bad in ["", "al_", "al_abc", "al_ABCDEFGHIJ", "al_abcdefghijk", "xx_abcdefghij", "al_abcdefghi-"] {
        assert!(AlertId::parse(bad).is_none(), "{bad}");
    }
}

#[test]
fn repeated_adapter_failures_within_a_minute_fold_into_one_with_a_count() {
    let (_home, log) = open();
    let v = version("a");
    let first = log.raise(new(AlertKind::AdapterFailed, &v, &["deadline"]), at("2026-10-08T10:00:00Z")).unwrap();
    let second = log.raise(new(AlertKind::AdapterFailed, &v, &["deadline"]), at("2026-10-08T10:00:40Z")).unwrap();
    // 60 s after the latest occurrence still folds, so a steady failure stays one alert.
    let third = log.raise(new(AlertKind::AdapterFailed, &v, &["deadline"]), at("2026-10-08T10:01:40Z")).unwrap();

    assert_eq!(second.id, first.id);
    assert_eq!(third.id, first.id);
    let all = log.list().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].count, 3);
    assert_eq!(all[0].at, "2026-10-08T10:01:40Z");
}

#[test]
fn a_failure_after_the_window_a_different_reason_or_another_version_is_its_own_alert() {
    let (_home, log) = open();
    let v = version("a");
    let t0 = at("2026-10-08T10:00:00Z");
    log.raise(new(AlertKind::AdapterFailed, &v, &["deadline"]), t0).unwrap();
    log.raise(new(AlertKind::AdapterFailed, &v, &["deadline"]), at("2026-10-08T10:01:01Z")).unwrap();
    log.raise(new(AlertKind::AdapterFailed, &v, &["memory"]), at("2026-10-08T10:01:02Z")).unwrap();
    log.raise(new(AlertKind::AdapterFailed, &version("b"), &["memory"]), at("2026-10-08T10:01:03Z")).unwrap();
    assert_eq!(log.list().unwrap().len(), 4);
}

#[test]
fn only_adapter_failures_fold_and_an_acknowledged_alert_does_not_absorb_new_ones() {
    let (_home, log) = open();
    let v = version("a");
    let t = at("2026-10-08T10:00:00Z");
    for _ in 0..3 {
        log.raise(new(AlertKind::Guardrail, &v, &["tool_call_added"]), t).unwrap();
    }
    assert_eq!(log.list().unwrap().len(), 3, "each guardrail event is its own alert");

    let failed = log.raise(new(AlertKind::AdapterFailed, &v, &["trap"]), t).unwrap();
    assert!(log.ack(&failed.id, t).unwrap());
    log.raise(new(AlertKind::AdapterFailed, &v, &["trap"]), at("2026-10-08T10:00:05Z")).unwrap();
    assert_eq!(log.list().unwrap().len(), 5);
}

#[test]
fn acknowledging_one_or_all() {
    let (_home, log) = open();
    let v = version("a");
    let t = at("2026-10-08T10:00:00Z");
    let a = log.raise(new(AlertKind::Refused, &v, &[]), t).unwrap();
    let b = log.raise(new(AlertKind::Quarantined, &v, &[]), t).unwrap();
    let c = log.raise(new(AlertKind::SourceMismatch, &v, &[]), t).unwrap();

    let later = at("2026-10-08T11:00:00Z");
    assert!(log.ack(&a.id, later).unwrap());
    assert!(!log.ack(&a.id, later).unwrap(), "already acknowledged");
    assert!(!log.ack(&AlertId::parse("al_zzzzzzzzzz").unwrap(), later).unwrap(), "unknown id");
    assert_eq!(log.unacked().unwrap(), 2);

    assert_eq!(log.ack_all(later).unwrap(), 2);
    assert_eq!(log.ack_all(later).unwrap(), 0);
    let all = log.list().unwrap();
    assert!(all.iter().all(|x| x.acked.as_deref() == Some("2026-10-08T11:00:00Z")));
    assert_eq!(all.iter().map(|x| &x.id).collect::<Vec<_>>(), [&a.id, &b.id, &c.id]);
}

#[test]
fn the_list_is_capped_and_acknowledged_alerts_go_first() {
    let (_home, log) = open();
    let t = at("2026-10-08T10:00:00Z");
    // Distinct versions keep them from folding, though these are not adapter_failed anyway.
    let first = log.raise(new(AlertKind::Refused, &version("0"), &[]), t).unwrap();
    let second = log.raise(new(AlertKind::Refused, &version("1"), &[]), t).unwrap();
    log.ack(&second.id, t).unwrap();
    for i in 2..MAX_ALERTS {
        log.raise(new(AlertKind::Refused, &version(&i.to_string()), &[]), t).unwrap();
    }
    assert_eq!(log.list().unwrap().len(), MAX_ALERTS, "exactly full");
    log.raise(new(AlertKind::Refused, &version("over"), &[]), t).unwrap();

    let all = log.list().unwrap();
    assert_eq!(all.len(), MAX_ALERTS);
    assert!(all.iter().any(|a| a.id == first.id), "the oldest open alert stays");
    assert!(all.iter().all(|a| a.id != second.id), "the acknowledged one went first");
}

#[test]
fn a_corrupt_file_is_an_error_not_an_empty_list() {
    let (home, log) = open();
    std::fs::write(home.path().join("adapters/alerts.toml"), "[[alert]\n").unwrap();
    assert!(log.list().is_err());
    assert!(log.raise(new(AlertKind::Refused, &version("a"), &[]), at("2026-10-08T10:00:00Z")).is_err());
}
