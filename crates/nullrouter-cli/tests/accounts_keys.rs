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
    // A command that exits without reading stdin closes the pipe first; that is not a failure.
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
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
    assert!(std::fs::read_to_string(&file).unwrap().starts_with("schema = 2\n"), "new files are schema 2");
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
fn a_schema_1_file_is_rewritten_as_schema_2() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(accounts::FILE);
    nullrouter_engine::files::write_private(
        &file,
        &format!("schema = 1\n[[account]]\nprovider = \"anthropic\"\nname = \"main\"\nsecret = \"{SECRET}\"\nhosts = [\"api.anthropic.com\"]\n"),
    )
    .unwrap();
    assert!(nr(dir.path(), &["accounts", "disable", "anthropic", "main"], "").status.success());
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("schema = 2\n") && text.contains("kind = \"key\""), "{text}");
    let list = Accounts::load(&file).unwrap();
    let a = list.get("anthropic", "main").unwrap();
    assert_eq!(a.kind, accounts::AccountKind::Key);
    assert!(a.disabled && a.secret.as_ref().unwrap().matches(SECRET));
    assert_eq!(mode(&file), 0o600);
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

/// T028: sign-in accounts in `accounts list`: `kind`, the access token's last four, and
/// `--long` (email, tier, expiry, last refresh). No token in full, in text or JSON.
#[test]
fn accounts_list_shows_sign_in_accounts() {
    use std::time::{Duration, UNIX_EPOCH};

    use nullrouter_engine::tokens::{self, Claims, TokenEntry};
    use nullrouter_registry::SecretString;

    const ACCESS: &str = "xai-access-SENTINEL-Zt1c";
    const REFRESH: &str = "xai-refresh-SENTINEL-r9r9";
    let dir = tempfile::tempdir().unwrap();
    let out = nr(dir.path(), &["accounts", "add", "anthropic", "api"], &format!("{SECRET}\n"));
    assert!(out.status.success());
    let mut list = Accounts::load(&dir.path().join(accounts::FILE)).unwrap();
    list.add(accounts::Account::signin("xai", "main", 0)).unwrap();
    list.save().unwrap();
    let at = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
    tokens::update(dir.path(), "xai", "main", |slot| {
        *slot = Some(TokenEntry {
            provider: "xai".into(),
            name: "main".into(),
            access_token: SecretString::new(ACCESS),
            refresh_token: Some(SecretString::new(REFRESH)),
            expires_at: at(1_790_000_000),
            scope: "openid".into(),
            claims: Claims { email: Some("alice@example.com".into()), user_id: None, tier: Some("SuperGrok".into()) },
            hosts: ["api.x.ai".to_owned()].into(),
            signed_in_at: at(1_789_990_000),
            last_refresh_at: Some(at(1_789_995_000)),
            state: None,
            state_since: None,
            state_reason: None,
        });
    })
    .unwrap();

    let text = String::from_utf8(nr(dir.path(), &["accounts", "list"], "").stdout).unwrap();
    let row = |p: &str| text.lines().find(|l| l.starts_with(p)).unwrap().split_whitespace().collect::<Vec<_>>();
    assert_eq!(row("anthropic")[..6], ["anthropic", "api", "key", "0", "1", "…0003"], "{text}");
    assert_eq!(row("xai")[..6], ["xai", "main", "signin", "0", "1", "…Zt1c"], "{text}");
    assert!(!text.contains("alice"), "email only with --long: {text}");

    let long = String::from_utf8(nr(dir.path(), &["accounts", "list", "--long"], "").stdout).unwrap();
    for want in ["alice@example.com", "SuperGrok", "2026-09-21T14:13:20Z", "2026-09-21T12:50:00Z"] {
        assert!(long.contains(want), "{want}: {long}");
    }

    let json = String::from_utf8(nr(dir.path(), &["--json", "accounts", "list", "xai"], "").stdout).unwrap();
    let rows: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    let r = &rows[0];
    assert_eq!((r["kind"].as_str(), r["secret"].as_str()), (Some("signin"), Some("…Zt1c")));
    assert_eq!((r["email"].as_str(), r["tier"].as_str()), (Some("alice@example.com"), Some("SuperGrok")));
    assert_eq!(r["expires_at"], "2026-09-21T14:13:20Z");
    assert_eq!(r["last_refresh_at"], "2026-09-21T12:50:00Z");
    for out in [&text, &long, &json] {
        assert!(!out.contains(ACCESS) && !out.contains(REFRESH) && !out.contains(SECRET), "no token in full");
    }
}

/// Writes tokens for sign-in account `provider/name` (added to `accounts.toml` too), with
/// a persisted state when given.
fn signed_in(
    home: &Path,
    provider: &str,
    name: &str,
    access: &str,
    state: Option<(nullrouter_engine::tokens::PersistedState, &str)>,
) {
    use std::time::{Duration, UNIX_EPOCH};

    use nullrouter_engine::tokens::{self, Claims, TokenEntry};
    use nullrouter_registry::SecretString;

    let mut list = Accounts::load(&home.join(accounts::FILE)).unwrap();
    list.add(accounts::Account::signin(provider, name, 0)).unwrap();
    list.save().unwrap();
    let at = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
    tokens::update(home, provider, name, |slot| {
        *slot = Some(TokenEntry {
            provider: provider.into(),
            name: name.into(),
            access_token: SecretString::new(access),
            refresh_token: Some(SecretString::new(format!("{access}-refresh"))),
            expires_at: at(4_102_444_800),
            scope: String::new(),
            claims: Claims::default(),
            hosts: Default::default(),
            signed_in_at: at(1_790_000_000),
            last_refresh_at: None,
            state: state.map(|(s, _)| s),
            // 2026-10-03T14:02:00Z
            state_since: state.map(|_| at(1_791_036_120)),
            state_reason: state.map(|(_, r)| r.to_owned()),
        });
    })
    .unwrap();
}

/// T059: the `accounts list` state column and hint line (contracts/operator-cli.md
/// § `accounts list`), read from the files when no server runs; `accounts enable` clears
/// `refused`.
#[test]
fn accounts_list_shows_sign_in_states_and_the_command() {
    use nullrouter_engine::tokens::{PersistedState, TokenStore};

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    assert!(
        nr(home, &["accounts", "add", "anthropic", "api", "--order", "1"], &format!("{SECRET}\n")).status.success()
    );
    signed_in(home, "anthropic", "max", "ant-access-SENTINEL-h3Kq", None);
    signed_in(home, "xai", "main", "xai-access-SENTINEL-Zt1c", Some((PersistedState::NeedsSignIn, "invalid_grant")));
    signed_in(
        home,
        "grok-cli",
        "work",
        "grok-access-SENTINEL-p0Lm",
        Some((PersistedState::Refused, "not for this client")),
    );

    let text = String::from_utf8(nr(home, &["accounts", "list"], "").stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    fn cells(l: &str) -> Vec<&str> {
        l.split("  ").map(str::trim).filter(|c| !c.is_empty()).collect()
    }
    assert_eq!(cells(lines[0]), ["provider", "name", "kind", "order", "priority", "secret", "state"], "{text}");
    assert_eq!(cells(lines[1]), ["anthropic", "api", "key", "1", "1", "…0003", "active"], "{text}");
    assert_eq!(cells(lines[2]), ["anthropic", "max", "signin", "0", "1", "…h3Kq", "active"], "{text}");
    assert_eq!(
        cells(lines[3]),
        ["xai", "main", "signin", "0", "1", "…Zt1c", "needs sign-in since 2026-10-03 14:02 (invalid_grant)"],
        "{text}"
    );
    assert_eq!(
        cells(lines[4]),
        [
            "grok-cli",
            "work",
            "signin",
            "0",
            "1",
            "…p0Lm",
            "refused by provider since 2026-10-03 14:02 (not for this client)"
        ],
        "{text}"
    );
    assert_eq!(
        lines[5..],
        ["  → run: nullrouter accounts signin xai main", "  → run: nullrouter accounts signin grok-cli work"],
        "{text}"
    );
    // The columns line up: the state column starts at the same character in every row.
    let state_at = |l: &str, s: &str| l.find(s).map(|i| l[..i].chars().count());
    assert_eq!(state_at(lines[0], "state"), state_at(lines[3], "needs sign-in"), "{text}");
    assert_eq!(state_at(lines[1], "active"), state_at(lines[4], "refused"), "{text}");

    let json = String::from_utf8(nr(home, &["--json", "accounts", "list", "xai"], "").stdout).unwrap();
    let rows: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(rows[0]["state"], "needs_sign_in");
    assert_eq!(rows[0]["state_since"], "2026-10-03T14:02:00Z");
    assert_eq!(rows[0]["state_reason"], "invalid_grant");

    // `enable` clears `refused`, not `needs sign-in` (only a sign-in does).
    let out = nr(home, &["accounts", "enable", "grok-cli", "work"], "");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("grok-cli/work: no longer marked refused"));
    assert!(nr(home, &["accounts", "enable", "xai", "main"], "").status.success());
    let store = TokenStore::load(home).unwrap();
    assert_eq!(store.get("grok-cli", "work").unwrap().state, None);
    assert_eq!(store.get("xai", "main").unwrap().state, Some(PersistedState::NeedsSignIn));
    let text = String::from_utf8(nr(home, &["accounts", "list"], "").stdout).unwrap();
    assert!(text.lines().any(|l| l.starts_with("grok-cli") && l.ends_with("  active")), "{text}");
    assert!(!text.contains("signin grok-cli work"), "{text}");
}

/// T064: `accounts remove` deletes the account's tokens under the lock; its quota history
/// stays (Clarifications Q4).
#[test]
fn removing_a_sign_in_account_deletes_its_tokens_and_keeps_its_history() {
    use nullrouter_engine::tokens::TokenStore;

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    signed_in(home, "xai", "main", "xai-access-SENTINEL-gone", None);
    signed_in(home, "xai", "other", "xai-access-SENTINEL-kept", None);
    let history = home.join("quota/xai");
    std::fs::create_dir_all(&history).unwrap();
    let files = [history.join("main.jsonl"), history.join("main.tally")];
    for f in &files {
        std::fs::write(f, "{}\n").unwrap();
    }

    let out = nr(home, &["accounts", "remove", "xai", "main"], "");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("xai/main: removed with its tokens"));
    let store = TokenStore::load(home).unwrap();
    assert!(store.get("xai", "main").is_none());
    assert!(store.get("xai", "other").is_some(), "other accounts keep theirs");
    let text = std::fs::read_to_string(home.join("tokens.toml")).unwrap();
    assert!(!text.contains("SENTINEL-gone"), "no token remains: {text}");
    assert_eq!(mode(&home.join("tokens.toml")), 0o600);
    for f in &files {
        assert!(f.exists(), "{} is kept", f.display());
    }
    assert!(Accounts::load(&home.join(accounts::FILE)).unwrap().get("xai", "main").is_none());

    // A key account has no tokens: nothing else is touched.
    assert!(nr(home, &["accounts", "add", "anthropic", "api"], &format!("{SECRET}\n")).status.success());
    let out = nr(home, &["accounts", "remove", "anthropic", "api"], "");
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("anthropic/api: "), "{out:?}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("tokens"));
}

#[test]
fn behaviour_show_prints_the_setting_and_whether_it_is_the_default() {
    let dir = tempfile::tempdir().unwrap();
    // No server runs, and a fresh home has no config.toml.
    let o = nr(dir.path(), &["behaviour", "show"], "");
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "break_behaviour  restart  (default)\n");
    let j = nr(dir.path(), &["--json", "behaviour", "show"], "");
    let j: serde_json::Value = serde_json::from_slice(&j.stdout).unwrap();
    assert_eq!(j, serde_json::json!({"break_behaviour": {"value": "restart", "default": true}}));

    let set = nr(dir.path(), &["behaviour", "set-break", "error_event"], "");
    assert!(set.status.success(), "{}", String::from_utf8_lossy(&set.stderr));
    let o = nr(dir.path(), &["behaviour", "show"], "");
    assert_eq!(String::from_utf8_lossy(&o.stdout), "break_behaviour  error_event\n");
    let j = nr(dir.path(), &["--json", "behaviour", "show"], "");
    let j: serde_json::Value = serde_json::from_slice(&j.stdout).unwrap();
    assert_eq!(j["break_behaviour"], serde_json::json!({"value": "error_event", "default": false}));
}

#[test]
fn behaviour_show_on_a_config_that_does_not_load_gives_the_error_and_no_value() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "schema = 1\n[[unified_model]\n").unwrap();
    let o = nr(dir.path(), &["behaviour", "show"], "");
    assert_eq!(o.status.code(), Some(1));
    assert!(o.stdout.is_empty());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.starts_with("startup failed:\n"), "{err}");
    assert!(err.contains("config.toml:2:"), "{err}");
    assert!(!err.contains("break_behaviour"), "{err}");
}

// ---- last used (spec 009, FR-029a) ----

/// The ids of the keys `keys list` shows, by name.
fn key_ids(home: &Path) -> std::collections::BTreeMap<String, String> {
    let rows: serde_json::Value = serde_json::from_slice(&nr(home, &["--json", "keys", "list"], "").stdout).unwrap();
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| (r["name"].as_str().unwrap().to_owned(), r["id"].as_str().unwrap().to_owned()))
        .collect()
}

/// A key's `last_used` in `keys list --json`.
fn last_used(home: &Path, id: &str) -> serde_json::Value {
    let rows: serde_json::Value = serde_json::from_slice(&nr(home, &["--json", "keys", "list"], "").stdout).unwrap();
    rows.as_array().unwrap().iter().find(|r| r["id"] == id).unwrap()["last_used"].clone()
}

/// The `arrived` of the newest record `records list` finds for the agent.
fn newest_arrival(home: &Path, id: &str) -> serde_json::Value {
    let out = nr(home, &["--json", "records", "list", "--agent", id, "--limit", "1"], "").stdout;
    let rows: serde_json::Value = serde_json::from_slice(&out).unwrap();
    rows.as_array().unwrap().first().map_or(serde_json::Value::Null, |r| r["arrived"].clone())
}

fn journal(home: &Path, lines: &[(&str, &str, &str)]) {
    std::fs::create_dir_all(home.join("records")).unwrap();
    for (day, id, agent) in lines {
        let at = format!("{day}T{}:00:00.250Z", &id[3..]);
        let line = serde_json::json!({"v":1,"t":"open","id":id,"arrived":at,"agent":agent,
            "style":"anthropic-messages","op":"generate","type":"text","target":"sonnet"});
        let path = home.join(format!("records/{day}.jsonl"));
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::write(path, format!("{old}{line}\n")).unwrap();
    }
}

struct Serving(std::process::Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn serve(home: &Path) -> Serving {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let serving = Serving(child);
    for _ in 0..500 {
        if home.join("run/operator.sock").exists() && std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return serving;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the server never came up");
}

#[test]
fn last_used_is_the_newest_arrival_of_the_keys_records_and_never_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    for name in ["busy", "idle"] {
        assert!(nr(h, &["keys", "issue", name], "").status.success());
    }
    let ids = key_ids(h);
    let (busy, idle) = (ids["busy"].as_str(), ids["idle"].as_str());
    assert_eq!(last_used(h, busy), serde_json::Value::Null, "no journal yet");

    // Two days; the newest arrival is on the later one, and written after an older one.
    journal(
        h,
        &[
            ("2026-10-01", "rq_09", busy),
            ("2026-10-02", "rq_11", busy),
            ("2026-10-02", "rq_10", "ak_not_listed"),
            ("2026-10-01", "rq_08", busy),
        ],
    );
    let text = String::from_utf8(nr(h, &["keys", "list"], "").stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].ends_with("2026-10-02T11:00:00.250Z"), "{text}");
    assert!(lines[1].ends_with("never"), "{text}");

    // Without a server, then with one: the same answer, and the same as `records list`.
    let answers = |h: &Path| (last_used(h, busy), last_used(h, idle), newest_arrival(h, busy));
    let (used, unused, arrival) = answers(h);
    assert_eq!(used, "2026-10-02T11:00:00.250Z");
    assert_eq!(used, arrival, "last used is the arrived of `records list --agent <id> --limit 1`");
    assert_eq!(unused, serde_json::Value::Null);
    let serving = serve(h);
    assert_eq!(answers(h), (used.clone(), unused.clone(), arrival.clone()), "with a server");

    // A record that arrives later moves it, through the server's cached index.
    journal(h, &[("2026-10-03", "rq_12", busy)]);
    assert_eq!(last_used(h, busy), "2026-10-03T12:00:00.250Z");

    // Forgetting the agent's records takes its last use with them, with the server's index warm.
    assert!(nr(h, &["records", "forget", "--agent", busy], "").status.success());
    assert_eq!(last_used(h, busy), serde_json::Value::Null);
    drop(serving);
    assert_eq!(last_used(h, busy), serde_json::Value::Null, "and without a server");
    assert!(String::from_utf8(nr(h, &["keys", "list"], "").stdout).unwrap().lines().all(|l| l.ends_with("never")));
}

#[test]
fn keys_bind_to_a_harness_and_the_list_shows_it_only_when_one_is() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    assert!(nr(h, &["keys", "issue", "plain"], "").status.success());
    let before = String::from_utf8(nr(h, &["keys", "list"], "").stdout).unwrap();
    assert!(!before.contains("hermes"), "{before}");

    assert!(nr(h, &["keys", "issue", "desk", "--harness", "hermes"], "").status.success());
    let file = Keys::load(&h.join(keys::FILE)).unwrap();
    let harness = |name: &str| file.iter().find(|k| k.name == name).unwrap().harness.clone();
    assert_eq!(harness("desk").map(|n| n.to_string()), Some("hermes".into()));
    assert!(harness("plain").is_none());
    let listed = String::from_utf8(nr(h, &["keys", "list"], "").stdout).unwrap();
    assert!(listed.contains("hermes"), "{listed}");
    let json: serde_json::Value = serde_json::from_slice(&nr(h, &["--json", "keys", "list"], "").stdout).unwrap();
    let row = |name: &str| json.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap().clone();
    assert_eq!(row("desk")["harness"], "hermes");
    assert!(row("plain").get("harness").is_none());

    // Rebind, then clear.
    assert!(nr(h, &["keys", "set-harness", "plain", "claude-code"], "").status.success());
    assert_eq!(
        Keys::load(&h.join(keys::FILE)).unwrap().iter().find(|k| k.name == "plain").unwrap().harness.as_ref().map(|n| n.to_string()),
        Some("claude-code".into())
    );
    assert!(nr(h, &["keys", "set-harness", "plain", "--clear"], "").status.success());
    assert!(Keys::load(&h.join(keys::FILE)).unwrap().iter().find(|k| k.name == "plain").unwrap().harness.is_none());

    // Reserved and malformed names, unknown keys, and a missing harness are refused.
    for args in [
        vec!["keys", "set-harness", "plain", "opencode"],
        vec!["keys", "set-harness", "plain", "Bad Name"],
        vec!["keys", "set-harness", "nobody", "hermes"],
        vec!["keys", "issue", "other", "--harness", "zcode"],
    ] {
        assert_eq!(nr(h, &args, "").status.code(), Some(1), "{args:?}");
    }
    assert!(!nr(h, &["keys", "set-harness", "plain"], "").status.success());
    assert!(Keys::load(&h.join(keys::FILE)).unwrap().iter().all(|k| k.name != "other"), "a refused issue leaves no key");
}
