use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use VersionState::*;
use nullrouter_adapters::HarnessName;
use nullrouter_adapters::fingerprint::{self, SourceFp};
use nullrouter_adapters::store::{
    Index, Origin, ReviewConfig, Store, StoreError, VersionEntry, VersionId, VersionState,
};

fn harness() -> HarnessName {
    HarnessName::new("claude-code").unwrap()
}

fn fp(seed: &str) -> SourceFp {
    fingerprint::of_files(&[("src/lib.rs".into(), seed.as_bytes().to_vec())])
}

fn entry(semver: &str, seed: &str) -> VersionEntry {
    let source_fp = fp(seed);
    VersionEntry {
        id: VersionId::new(&semver.parse().unwrap(), &source_fp),
        semver: semver.into(),
        state: Queued,
        source_fp,
        wasm_hash: None,
        kit_abi: None,
        submitted: "2026-09-28T10:00:00Z".into(),
        state_reason: String::new(),
        rebuilding: false,
        rebuild_failed: false,
        origin: Origin::Local("/tmp/pkg".into()),
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// The edges of the state machine in data-model.md, written out independently of the code.
const EDGES: &[(VersionState, VersionState)] = &[
    (Queued, Building),
    (Queued, Refused),
    (Building, InReview),
    (Building, Refused),
    (InReview, Reported),
    (InReview, Quarantined),
    (Quarantined, InReview),
    (Reported, Approved),
    (Reported, Rejected),
    (Approved, Suspect),
    (Approved, Superseded),
    (Suspect, Approved),
    (Suspect, Superseded),
];

#[test]
fn every_edge_is_allowed_and_every_other_pair_is_refused() {
    for from in VersionState::ALL {
        for to in VersionState::ALL {
            let expected = EDGES.contains(&(from, to));
            assert_eq!(from.can_become(to), expected, "{from} -> {to}");
        }
    }
}

#[test]
fn terminal_states_have_no_way_out() {
    for from in VersionState::ALL {
        let leaves = VersionState::ALL.iter().any(|&to| from.can_become(to));
        assert_eq!(from.is_terminal(), !leaves, "{from}");
    }
}

#[test]
fn a_transition_through_the_index_refuses_a_forbidden_edge_and_changes_nothing() {
    let mut index = Index::default();
    let v = entry("0.3.0", "a");
    let id = v.id.clone();
    index.submit(&harness(), v).unwrap();
    index.transition(&harness(), &id, Building, "").unwrap();
    index.transition(&harness(), &id, InReview, "").unwrap();

    // Approving from in_review is refused.
    let err = index.transition(&harness(), &id, Approved, "").unwrap_err();
    assert!(matches!(err, StoreError::Transition { from: InReview, to: Approved }), "{err}");
    assert_eq!(index.version(&harness(), &id).unwrap().state, InReview);
    assert!(index.approve(&harness(), &id).is_err());
    assert!(index.harness(&harness()).unwrap().active.is_none());

    index.transition(&harness(), &id, Quarantined, "no_review_model").unwrap();
    assert_eq!(index.version(&harness(), &id).unwrap().state_reason, "no_review_model");
}

#[test]
fn approving_activates_the_version_and_supersedes_the_one_before() {
    let mut index = Index::default();
    let (first, second) = (entry("0.3.0", "a"), entry("0.4.0", "b"));
    let (a, b) = (first.id.clone(), second.id.clone());
    for v in [first, second] {
        let id = v.id.clone();
        index.submit(&harness(), v).unwrap();
        for to in [Building, InReview, Reported] {
            index.transition(&harness(), &id, to, "").unwrap();
        }
    }
    index.approve(&harness(), &a).unwrap();
    assert_eq!(index.serving(&harness()).map(|v| &v.id), Some(&a));

    index.approve(&harness(), &b).unwrap();
    assert_eq!(index.serving(&harness()).map(|v| &v.id), Some(&b));
    assert_eq!(index.version(&harness(), &a).unwrap().state, Superseded);
}

#[test]
fn a_suspect_version_stays_active_but_does_not_serve_until_cleared() {
    let mut index = Index::default();
    let v = entry("0.3.0", "a");
    let id = v.id.clone();
    index.submit(&harness(), v).unwrap();
    for to in [Building, InReview, Reported] {
        index.transition(&harness(), &id, to, "").unwrap();
    }
    index.approve(&harness(), &id).unwrap();

    index.transition(&harness(), &id, Suspect, "guardrail").unwrap();
    assert_eq!(index.harness(&harness()).unwrap().active.as_ref(), Some(&id));
    assert!(index.serving(&harness()).is_none());

    index.transition(&harness(), &id, Approved, "").unwrap();
    assert!(index.serving(&harness()).is_some());
}

#[test]
fn a_duplicate_submission_is_refused_and_a_submission_always_starts_queued() {
    let mut index = Index::default();
    let mut v = entry("0.3.0", "a");
    v.state = Approved;
    index.submit(&harness(), v.clone()).unwrap();
    assert_eq!(index.version(&harness(), &v.id).unwrap().state, Queued);
    assert!(matches!(index.submit(&harness(), v), Err(StoreError::DuplicateVersion { .. })));
}

#[test]
fn a_version_id_is_v_semver_and_the_first_eight_hex_of_the_fingerprint() {
    let fp = fp("a");
    let hex = fp.as_str().strip_prefix("sha256:").unwrap();
    let id = VersionId::new(&"0.3.0".parse().unwrap(), &fp);
    assert_eq!(id.as_str(), format!("v0.3.0-{}", &hex[..8]));
    // The same version with different source gets a different id.
    assert_ne!(id, VersionId::new(&"0.3.0".parse().unwrap(), &self::fp("b")));
}

#[test]
fn the_index_round_trips_through_its_file() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    assert_eq!(store.load_index().unwrap(), Index::default(), "no file is an empty index");

    let review = ReviewConfig { model: "claude-sonnet".into(), budget_tokens: 60_000, reserve_output: 4096 };
    let mut index = Index { review: Some(review), ..Index::default() };
    let mut v = entry("0.3.0", "a");
    v.wasm_hash = Some("sha256:abc".into());
    v.kit_abi = Some(1);
    v.rebuilding = true;
    v.origin = Origin::Catalogue("https://example.com/claude-code-0.3.0.tar.gz".into());
    let id = v.id.clone();
    index.submit(&harness(), v).unwrap();
    index.transition(&harness(), &id, Refused, "gate: unsafe").unwrap();
    index.submit(&HarnessName::new("other").unwrap(), entry("1.0.0", "z")).unwrap();

    store.save_index(&index).unwrap();
    assert_eq!(store.load_index().unwrap(), index);
}

#[test]
fn the_documented_index_text_loads() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let text = format!(
        r#"schema = 1
[review]
model = "claude-sonnet"
budget_tokens = 60000

[[harness]]
name = "claude-code"
active = "v0.3.0-1a2b3c4d"
source = "catalogue"

[[harness.version]]
id = "v0.3.0-1a2b3c4d"
semver = "0.3.0"
state = "approved"
source_fp = "sha256:{}"
wasm_hash = "sha256:abc"
kit_abi = 1
origin = {{ catalogue = "https://example.com/claude-code-0.3.0.tar.gz" }}
submitted = "2026-09-28T10:00:00Z"
state_reason = ""
"#,
        "1a2b3c4d".repeat(8)
    );
    std::fs::write(store.root().join("index.toml"), text).unwrap();
    let index = store.load_index().unwrap();
    assert_eq!(index.review.as_ref().unwrap().reserve_output, 4096, "the default reserve");
    assert_eq!(index.serving(&harness()).unwrap().semver, "0.3.0");
}

#[test]
fn an_index_that_does_not_parse_or_has_another_schema_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let path = store.root().join("index.toml");
    std::fs::write(&path, "schema = [").unwrap();
    assert!(matches!(store.load_index(), Err(StoreError::BadIndex { .. })));
    std::fs::write(&path, "schema = 2\n").unwrap();
    assert!(matches!(store.load_index(), Err(StoreError::BadIndex { .. })));
}

#[test]
fn a_write_replaces_the_file_whole_and_leaves_no_temporary_behind() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    store.save_index(&Index::default()).unwrap();
    let mut index = Index::default();
    index.submit(&harness(), entry("0.3.0", "a")).unwrap();
    store.save_index(&index).unwrap();
    assert_eq!(store.load_index().unwrap(), index);

    let names: Vec<_> =
        std::fs::read_dir(store.root()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert_eq!(names, ["index.toml"]);
}

#[test]
fn files_are_mode_0600_and_directories_0700() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let v = entry("0.3.0", "a");
    store.write_version_file(&harness(), &v.id, "source/src/lib.rs", b"fn main() {}").unwrap();
    store.write_version_file(&harness(), &v.id, "module.wasm", b"\0asm").unwrap();
    store.save_index(&Index::default()).unwrap();

    let dir = store.version_dir(&harness(), &v.id);
    for d in [store.root(), &store.root().join("claude-code"), &dir, &dir.join("source"), &dir.join("source/src")] {
        assert_eq!(mode(d), 0o700, "{}", d.display());
    }
    for f in [dir.join("source/src/lib.rs"), dir.join("module.wasm"), store.root().join("index.toml")] {
        assert_eq!(mode(&f), 0o600, "{}", f.display());
    }
}

#[test]
fn a_version_file_path_that_leaves_the_version_directory_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let v = entry("0.3.0", "a");
    for bad in ["", "../escape", "/etc/passwd", "source/../../x"] {
        let err = store.write_version_file(&harness(), &v.id, bad, b"x").unwrap_err();
        assert!(matches!(err, StoreError::BadPath(_)), "{bad:?}: {err}");
    }
}

#[test]
fn opening_creates_a_private_adapters_directory_and_refuses_a_group_readable_one() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    assert_eq!(mode(store.root()), 0o700);
    Store::open(home.path()).expect("an owner-only directory opens again");

    for loose in [0o750, 0o705, 0o755, 0o770] {
        std::fs::set_permissions(store.root(), std::fs::Permissions::from_mode(loose)).unwrap();
        let err = Store::open(home.path()).unwrap_err();
        assert!(matches!(err, StoreError::NotPrivate { mode, .. } if mode == loose), "{loose:o}: {err}");
    }
}
