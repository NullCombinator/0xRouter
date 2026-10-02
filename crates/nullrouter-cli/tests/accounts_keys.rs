//! `accounts` and `keys` (T057) through the binary: secrets come from stdin or an
//! environment variable name, never argv, and both files stay private.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use nullrouter_engine::accounts::{self, Accounts, SecretSource};
use nullrouter_engine::keys::{self, BreakBehaviour, Keys};

const SECRET: &str = "sk-cli-SENTINEL-0003";

fn nr(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn accounts_take_the_secret_from_stdin_and_stay_private() {
    let dir = tempfile::tempdir().unwrap();
    let out = nr(dir.path(), &["accounts", "add", "anthropic", "main"], &format!("{SECRET}\n"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let file = dir.path().join(accounts::FILE);
    assert_eq!(mode(&file), 0o600);
    let list = Accounts::load(&file).unwrap();
    let a = list.get("anthropic", "main").unwrap();
    assert_eq!(a.source, SecretSource::Literal);
    assert!(a.secret.as_ref().unwrap().with_exposed(|s| s == SECRET), "the trailing newline is not part of the secret");
    assert!(a.hosts.contains("api.anthropic.com"), "bound to the provider's hosts: {:?}", a.hosts);

    let out = nr(dir.path(), &["accounts", "add", "anthropic", "spare", "--env", "NR_TEST_UNSET_VAR"], "");
    assert!(out.status.success());
    let out = nr(dir.path(), &["--json", "accounts", "list"], "");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains(SECRET), "listings never show a secret");
    let rows: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(rows[0]["secret"], "…0003");
    assert_eq!(rows[1]["secret"], "env:NR_TEST_UNSET_VAR");

    assert!(nr(dir.path(), &["accounts", "disable", "anthropic", "main"], "").status.success());
    assert!(Accounts::load(&file).unwrap().get("anthropic", "main").unwrap().disabled);
    assert!(nr(dir.path(), &["accounts", "remove", "anthropic", "spare"], "").status.success());
    assert!(Accounts::load(&file).unwrap().get("anthropic", "spare").is_none());
}

#[test]
fn accounts_refuse_bad_input() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(nr(dir.path(), &["accounts", "add", "anthropic", "main"], "\n").status.code(), Some(1), "empty secret");
    assert_eq!(nr(dir.path(), &["accounts", "add", "no-such-provider", "main"], SECRET).status.code(), Some(1));
    assert_eq!(nr(dir.path(), &["accounts", "add", "anthropic", "Bad Name"], SECRET).status.code(), Some(1));
    assert_eq!(nr(dir.path(), &["accounts", "remove", "anthropic", "main"], "").status.code(), Some(1));
    assert!(!dir.path().join(accounts::FILE).exists());
}

#[test]
fn keys_are_printed_once_and_stored_as_digests() {
    let dir = tempfile::tempdir().unwrap();
    let out = nr(dir.path(), &["keys", "issue", "laptop", "--break", "error_event"], "");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let key = String::from_utf8(out.stdout).unwrap().trim().to_owned();
    assert!(key.starts_with(keys::PREFIX), "{key}");
    let file = dir.path().join(keys::FILE);
    assert_eq!(mode(&file), 0o600);
    assert!(!std::fs::read_to_string(&file).unwrap().contains(&key));
    let k = Keys::load(&file).unwrap().lookup(&key).unwrap().clone();
    assert_eq!(k.break_behaviour, Some(BreakBehaviour::ErrorEvent));

    let listed = String::from_utf8(nr(dir.path(), &["keys", "list"], "").stdout).unwrap();
    assert!(!listed.contains(&key) && listed.contains("laptop"));
    assert_eq!(nr(dir.path(), &["keys", "issue", "laptop"], "").status.code(), Some(1), "names are unique");
    assert_eq!(nr(dir.path(), &["keys", "issue", "x", "--break", "sometimes"], "").status.code(), Some(1));

    assert!(nr(dir.path(), &["keys", "set-break", "laptop", "default"], "").status.success());
    assert_eq!(Keys::load(&file).unwrap().lookup(&key).unwrap().break_behaviour, None);
    assert!(nr(dir.path(), &["keys", "revoke", &k.id], "").status.success());
    assert!(Keys::load(&file).unwrap().lookup(&key).is_none(), "a revoked key no longer authenticates");
}
