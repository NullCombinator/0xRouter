use std::fs::{self, File};
use std::os::unix::fs::symlink;
use std::time::{Duration, SystemTime};

use nullrouter_adapters::HarnessName;
use nullrouter_adapters::fingerprint::{self, FingerprintError, SourceFp};
use nullrouter_adapters::store::{Origin, Store, StoreError, VersionEntry, VersionId, VersionState};

fn write(dir: &std::path::Path, relative: &str, bytes: &[u8]) {
    let path = dir.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, bytes) in files {
        write(dir.path(), path, bytes.as_bytes());
    }
    dir
}

const FILES: [(&str, &str); 3] =
    [("adapter.toml", "harness = \"x\""), ("src/lib.rs", "fn a() {}"), ("src/more/b.rs", "fn b() {}")];

#[test]
fn the_fingerprint_is_stable_across_file_order_and_mtime() {
    let forward = tree(&FILES);
    let reverse = tree(&[FILES[2], FILES[1], FILES[0]]);
    // Make the second tree's files look years older.
    let old = SystemTime::now() - Duration::from_secs(86_400 * 900);
    for f in ["adapter.toml", "src/lib.rs", "src/more/b.rs"] {
        File::options().write(true).open(reverse.path().join(f)).unwrap().set_modified(old).unwrap();
    }
    let a = fingerprint::of_dir(forward.path()).unwrap();
    assert_eq!(a, fingerprint::of_dir(reverse.path()).unwrap());
    assert!(SourceFp::parse(a.as_str()).is_ok(), "{a}");
    // The in-memory form agrees with the directory form, whatever order it is given in.
    let owned: Vec<(String, Vec<u8>)> =
        FILES.iter().rev().map(|(p, b)| ((*p).to_owned(), b.as_bytes().to_vec())).collect();
    assert_eq!(fingerprint::of_files(&owned), a);
}

#[test]
fn any_changed_byte_rename_or_added_file_changes_it() {
    let base = fingerprint::of_dir(tree(&FILES).path()).unwrap();

    let mut changed = FILES;
    changed[1] = ("src/lib.rs", "fn a() {} ");
    assert_ne!(fingerprint::of_dir(tree(&changed).path()).unwrap(), base, "one byte");

    let mut renamed = FILES;
    renamed[1] = ("src/lib2.rs", "fn a() {}");
    assert_ne!(fingerprint::of_dir(tree(&renamed).path()).unwrap(), base, "a rename");

    let added = tree(&FILES);
    write(added.path(), "src/extra.rs", b"");
    assert_ne!(fingerprint::of_dir(added.path()).unwrap(), base, "an added empty file");
}

#[test]
fn the_path_and_length_framing_keeps_neighbouring_files_apart() {
    // Moving a byte from one file to the next must not give the same hash.
    let a = fingerprint::of_files(&[("a".into(), b"xy".to_vec()), ("b".into(), b"z".to_vec())]);
    let b = fingerprint::of_files(&[("a".into(), b"x".to_vec()), ("b".into(), b"yz".to_vec())]);
    assert_ne!(a, b);
    // A path and its content can't be confused either.
    let c = fingerprint::of_files(&[("a".into(), b"\0".to_vec())]);
    let d = fingerprint::of_files(&[("a\0".into(), Vec::new())]);
    assert_ne!(c, d);
}

#[test]
fn empty_directories_are_not_part_of_it_but_a_symlink_is_refused() {
    let t = tree(&FILES);
    let base = fingerprint::of_dir(t.path()).unwrap();
    fs::create_dir(t.path().join("empty")).unwrap();
    assert_eq!(fingerprint::of_dir(t.path()).unwrap(), base);

    symlink("/etc/passwd", t.path().join("link")).unwrap();
    assert!(matches!(fingerprint::of_dir(t.path()), Err(FingerprintError::NotRegular(_))));
}

#[test]
fn a_malformed_fingerprint_does_not_parse() {
    let (upper, short) = (format!("sha256:{}", "A".repeat(64)), format!("sha256:{}", "a".repeat(63)));
    for bad in ["", "sha256:", "sha256:abc", "md5:00", upper.as_str(), short.as_str()] {
        assert!(SourceFp::parse(bad).is_err(), "{bad:?}");
    }
    assert!(SourceFp::parse(&format!("sha256:{}", "a".repeat(64))).is_ok());
}

/// A built version stored under `home`: its source, its module, and the entry that names them.
fn stored(home: &std::path::Path) -> (Store, HarnessName, VersionEntry) {
    let store = Store::open(home).unwrap();
    let harness = HarnessName::new("claude-code").unwrap();
    let source = tree(&FILES);
    let source_fp = fingerprint::of_dir(source.path()).unwrap();
    let id = VersionId::new(&"0.3.0".parse().unwrap(), &source_fp);
    for (path, bytes) in FILES {
        store.write_version_file(&harness, &id, &format!("source/{path}"), bytes.as_bytes()).unwrap();
    }
    let wasm = b"\0asm\x01\0\0\0";
    store.write_version_file(&harness, &id, "module.wasm", wasm).unwrap();
    let entry = VersionEntry {
        id,
        semver: "0.3.0".into(),
        state: VersionState::Approved,
        source_fp,
        wasm_hash: Some(nullrouter_sandbox::wasm_hash(wasm)),
        kit_abi: Some(1),
        submitted: "2026-09-28T10:00:00Z".into(),
        state_reason: String::new(),
        rebuilding: false,
        rebuild_failed: false,
        origin: Origin::Local("/tmp/pkg".into()),
    };
    (store, harness, entry)
}

#[test]
fn an_untouched_version_verifies() {
    let home = tempfile::tempdir().unwrap();
    let (store, harness, entry) = stored(home.path());
    store.verify(&harness, &entry).unwrap();
}

#[test]
fn editing_the_source_or_the_module_after_the_fact_is_a_source_mismatch() {
    for (file, what) in [("source/src/lib.rs", "the source"), ("module.wasm", "the module")] {
        let home = tempfile::tempdir().unwrap();
        let (store, harness, entry) = stored(home.path());
        let path = store.version_dir(&harness, &entry.id).join(file);
        let mut bytes = fs::read(&path).unwrap();
        bytes.push(b' ');
        fs::write(&path, bytes).unwrap();
        match store.verify(&harness, &entry) {
            Err(StoreError::SourceMismatch { what: got, .. }) => assert_eq!(got, what, "{file}"),
            other => panic!("{file}: {other:?}"),
        }
    }
}

#[test]
fn a_file_added_removed_or_missing_is_a_source_mismatch_too() {
    let home = tempfile::tempdir().unwrap();
    let (store, harness, entry) = stored(home.path());
    let dir = store.version_dir(&harness, &entry.id);

    write(&dir, "source/src/extra.rs", b"");
    assert!(matches!(store.verify(&harness, &entry), Err(StoreError::SourceMismatch { .. })));
    fs::remove_file(dir.join("source/src/extra.rs")).unwrap();
    store.verify(&harness, &entry).unwrap();

    fs::remove_file(dir.join("module.wasm")).unwrap();
    assert!(matches!(store.verify(&harness, &entry), Err(StoreError::SourceMismatch { what: "the module", .. })));
    fs::remove_dir_all(dir.join("source")).unwrap();
    assert!(matches!(store.verify(&harness, &entry), Err(StoreError::SourceMismatch { what: "the source", .. })));
}
