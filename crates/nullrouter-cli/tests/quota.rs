//! `nullrouter quota` (spec 005 T069; contracts/operator-cli.md § `quota`): the text view of a
//! `quota.list` answer, line for line; `quota poll` needs the running server (exit 4);
//! `quota interval` saves `poll_interval` and says it applies at the next start; `quota` with
//! no server names the accounts whose quota isn't reported. `quota history`, `prune` and
//! `forget` (T083) work on the history files when no server runs; `accounts remove` keeps the
//! history.

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, UNIX_EPOCH};

use nullrouter_cli::quota_text;
use serde_json::{Value, json};

/// 2026-10-03 14:23:00 UTC, a Saturday.
const NOW: u64 = 1_791_037_380;

fn at(hm: &str) -> String {
    format!("2026-10-03T{hm}:00Z")
}

fn poll(provider: &str, name: &str, hm: &str, windows: Value) -> Value {
    json!({"provider": provider, "account": name, "at": at(hm), "windows": windows, "retry": false})
}

/// The contract's example as the server answers `quota.list`.
fn answer() -> Vec<Value> {
    let pct = |name: &str, used: f64, resets: &str| json!({"name": name, "unit": "percent", "used": used, "limit": 100.0, "remaining": 100.0 - used, "resets_at": resets});
    vec![
        json!({"provider": "anthropic", "name": "max", "kind": "signin", "reported": true, "interval_s": 600,
            "latest": poll("anthropic", "max", "14:20", json!([
                pct("5-hour", 38.0, "2026-10-03T16:00:00.000Z"),
                pct("weekly", 19.0, "2026-10-08T09:00:00.000Z"),
            ])),
            "last_failure": null}),
        json!({"provider": "grok-cli", "name": "work", "kind": "signin", "reported": true, "interval_s": 600,
            "latest": poll("grok-cli", "work", "14:18", json!([
                {"name": "monthly included", "unit": "credits", "used": 1240.0, "limit": 5000.0, "remaining": 3760.0,
                 "resets_at": "2026-11-01T00:00:00.000Z"},
            ])),
            "last_failure": {"provider": "grok-cli", "account": "work", "at": at("14:08"), "windows": [], "retry": false,
                "error": {"class": "timeout", "reason": "no answer within 30 s", "summary": "timeout"}}}),
        json!({"provider": "opencode-go", "name": "main", "kind": "key", "reported": true, "interval_s": 600,
            "latest": poll("opencode-go", "main", "14:21", json!([pct("rolling", 5.0, "2026-10-03T18:30:00.000Z")])),
            "last_failure": null}),
        json!({"provider": "xai", "name": "main", "kind": "signin", "reported": false, "interval_s": 600,
            "latest": null, "last_failure": null}),
    ]
}

#[test]
fn quota_text_is_the_contracts() {
    let now = UNIX_EPOCH + Duration::from_secs(NOW);
    // Every account line carries its poll's age and interval, so the contract's shortened
    // second and third lines read in full here.
    let want = "\
anthropic/max            polled 14:20 (3 min ago, every 10 min)
  5-hour                 62% left   resets 16:00
  weekly                 81% left   resets Thu 09:00
grok-cli/work            polled 14:18 (5 min ago, every 10 min); last poll failed 14:08 (timeout)
  monthly included       1,240 / 5,000 credits used   resets Nov 1
opencode-go/main         polled 14:21 (2 min ago, every 10 min)
  rolling                95% left   resets 18:30
xai/main                 quota not reported
";
    assert_eq!(quota_text::render(&answer(), now, 0), want);
}

#[test]
fn edge_lines() {
    let now = UNIX_EPOCH + Duration::from_secs(NOW);
    let a = vec![
        json!({"provider": "p", "name": "never", "reported": true, "interval_s": 900, "latest": null, "last_failure": null}),
        json!({"provider": "p", "name": "failing", "reported": true, "interval_s": 600, "latest": null,
            "last_failure": {"at": "2026-10-02T23:59:00Z", "error": {"summary": "HTTP 500"}}}),
        json!({"provider": "p", "name": "over", "reported": true, "interval_s": 3600,
            "latest": {"at": "2026-10-03T12:23:00Z", "windows": [
                {"name": "weekly", "unit": "percent", "used": 130.0, "limit": 100.0, "remaining": 0.0},
                {"name": "prepaid", "unit": "credits", "remaining": 500.0},
            ]},
            "last_failure": null}),
    ];
    let want = "\
p/never                  not polled yet
p/failing                not polled yet; last poll failed Oct 2 23:59 (HTTP 500)
p/over                   polled 12:23 (2 h ago, every 1 h)
  weekly                 0% left (130% used)
  prepaid                500 credits left
";
    assert_eq!(quota_text::render(&a, now, 0), want);
    // The same instant at UTC+2 is past midnight.
    assert!(quota_text::render(&a[1..2], now, 7200).contains("failed 01:59"));
}

fn nr(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nullrouter")).arg("--home").arg(home).args(args).output().unwrap()
}

fn text(o: &Output) -> (String, String) {
    (String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

fn home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let accounts = "schema = 2\n\
        [[account]]\nprovider = \"opencode-go\"\nname = \"main\"\nsecret = \"sk-go-SENTINEL-1\"\n\
        [[account]]\nprovider = \"xai\"\nname = \"main\"\nsecret = \"xai-SENTINEL-2\"\n";
    nullrouter_engine::files::write_private(&dir.path().join("accounts.toml"), accounts).unwrap();
    dir
}

#[test]
fn quota_poll_needs_the_server() {
    let dir = home();
    let o = nr(dir.path(), &["quota", "poll", "opencode-go", "main"]);
    assert_eq!(o.status.code(), Some(4), "{:?}", text(&o));
    assert!(text(&o).1.contains("no server is running"), "{:?}", text(&o));
}

#[test]
fn quota_interval_saves_for_the_next_start() {
    let dir = home();
    let o = nr(dir.path(), &["quota", "interval", "opencode-go", "main", "15m"]);
    assert!(o.status.success(), "{:?}", text(&o));
    assert_eq!(text(&o).0, "opencode-go/main: polls every 15 min; saved; applies at next start\n");
    let file = std::fs::read_to_string(dir.path().join("accounts.toml")).unwrap();
    assert!(file.contains("poll_interval = \"15m\""), "{file}");
    assert_eq!(file.matches("poll_interval").count(), 1, "only that account: {file}");

    let o = nr(dir.path(), &["quota", "interval", "opencode-go", "main", "30s"]);
    assert_eq!(text(&o).0, "opencode-go/main: polls every 2 min (raised to the floor); saved; applies at next start\n");
    let o = nr(dir.path(), &["quota", "interval", "opencode-go", "main", "default"]);
    assert_eq!(text(&o).0, "opencode-go/main: polls every 10 min; saved; applies at next start\n");
    assert!(!std::fs::read_to_string(dir.path().join("accounts.toml")).unwrap().contains("poll_interval"));

    let o = nr(dir.path(), &["quota", "interval", "opencode-go", "nobody", "15m"]);
    assert_eq!(o.status.code(), Some(1));
    let o = nr(dir.path(), &["quota", "interval", "opencode-go", "main", "soon"]);
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn quota_without_a_server_names_what_isnt_reported() {
    let dir = home();
    let o = nr(dir.path(), &["quota"]);
    assert!(o.status.success(), "{:?}", text(&o));
    let (out, err) = text(&o);
    assert_eq!(out, "opencode-go/main         not polled yet\nxai/main                 quota not reported\n");
    assert!(err.contains("no server is running"), "{err}");
    let o = nr(dir.path(), &["quota", "xai"]);
    assert_eq!(text(&o).0, "xai/main                 quota not reported\n");
    assert!(!format!("{:?}", text(&o)).contains("SENTINEL"));
}

/// Two kept polls of `opencode-go/main` (Oct 1 and Oct 3) and one of `xai/main`.
fn with_history(dir: &Path) {
    use nullrouter_engine::quota::history::{self, Entry, EntryError};
    use nullrouter_engine::quota::tally::{AccountTally, ModelTally};
    let tally = AccountTally::from([(
        "deepseek-flash".to_owned(),
        ModelTally {
            requests: 3,
            requests_usage_unreported: 1,
            input: 1200,
            output: 340,
            cache_read: 4096,
            cache_write: 0,
        },
    )]);
    let window: nullrouter_engine::quota::QuotaWindow = serde_json::from_value(
        json!({"name": "rolling", "unit": "percent", "used": 25.0, "limit": 100.0, "remaining": 75.0}),
    )
    .unwrap();
    let entry = |at: &str, ok: bool, tally: AccountTally| Entry {
        v: 1,
        at: at.into(),
        ok,
        error: (!ok).then(|| EntryError { class: "status".into(), status: Some(500), reason: "down".into() }),
        windows: if ok { vec![window.clone()] } else { Vec::new() },
        tally,
    };
    history::append(dir, "opencode-go", "main", &entry("2026-10-01T10:00:00.000Z", true, tally)).unwrap();
    history::append(dir, "opencode-go", "main", &entry("2026-10-03T14:20:00.000Z", false, AccountTally::new()))
        .unwrap();
    history::append(dir, "xai", "main", &entry("2026-10-01T09:00:00.000Z", true, AccountTally::new())).unwrap();
}

#[test]
fn quota_history_reads_the_kept_polls() {
    let dir = home();
    with_history(dir.path());
    let o = nr(dir.path(), &["quota", "history", "opencode-go", "main"]);
    assert!(o.status.success(), "{:?}", text(&o));
    let (out, _) = text(&o);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out}");
    assert!(lines[0].ends_with("ok       rolling 75% left"), "{out}");
    assert_eq!(
        lines[1],
        "  deepseek-flash         3 requests (1 usage unreported)   input 1,200   output 340   cache read 4,096   cache write 0"
    );
    assert!(lines[2].ends_with("failed   HTTP 500   no traffic"), "{out}");

    let o = nr(dir.path(), &["--json", "quota", "history", "opencode-go", "main", "--limit", "1"]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["v"], 1);
    assert_eq!(v[0]["error"]["status"], 500);
    let o = nr(dir.path(), &["--json", "quota", "history", "opencode-go", "main", "--since", "2026-10-02"]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1, "{v}");
    let o = nr(dir.path(), &["quota", "history", "opencode-go", "nobody"]);
    assert!(o.status.success());
    assert!(text(&o).1.contains("no poll history"), "{:?}", text(&o));
    let o = nr(dir.path(), &["quota", "history", "opencode-go", "main", "--since", "soon"]);
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn quota_prune_and_forget_edit_the_files() {
    let dir = home();
    with_history(dir.path());
    let o = nr(dir.path(), &["quota", "prune", "--before", "2026-10-02", "opencode-go"]);
    assert!(o.status.success(), "{:?}", text(&o));
    assert_eq!(text(&o).0, "deleted 1 history entry older than 2026-10-02T00:00:00Z\n");
    let left = |p: &str| nullrouter_engine::quota::history::read(dir.path(), p, "main", None, None).unwrap().len();
    assert_eq!((left("opencode-go"), left("xai")), (1, 1), "only the named provider");
    let o = nr(dir.path(), &["quota", "prune", "--before", "2026-10-02"]);
    assert_eq!(text(&o).0, "deleted 1 history entry older than 2026-10-02T00:00:00Z\n");
    assert_eq!(left("xai"), 0);

    // `accounts remove` keeps the history; `quota forget` deletes it.
    let o = nr(dir.path(), &["accounts", "remove", "opencode-go", "main"]);
    assert!(o.status.success(), "{:?}", text(&o));
    assert_eq!(left("opencode-go"), 1, "kept after accounts remove");
    let o = nr(dir.path(), &["quota", "forget", "opencode-go", "main"]);
    assert_eq!(text(&o).0, "opencode-go/main: poll history deleted\n");
    assert_eq!(left("opencode-go"), 0);
    let o = nr(dir.path(), &["quota", "forget", "opencode-go", "main"]);
    assert_eq!(text(&o).0, "opencode-go/main: no poll history\n");
    let o = nr(dir.path(), &["quota", "prune", "--before", "yesterday"]);
    assert_eq!(o.status.code(), Some(1));
}
