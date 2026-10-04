//! Poll history (spec 005 T080, US5, FR-023, FR-025, Clarifications Q4; research R15): one
//! JSON line per poll with `v = 1` and the tally since the previous entry; files 0600 and
//! directories 0700; the running tally is checkpointed every 10 s (shortened here) and at
//! shutdown, so a graceful restart loses nothing; `prune --before` removes only older
//! entries; `forget` deletes one account's files; removing an account stops its polls but
//! keeps its history.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::*;
use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::maintenance::{self, JobKind};
use nullrouter_engine::quota::history::{self, Entry};
use nullrouter_engine::quota::poll::QuotaPoll;
use nullrouter_engine::quota::tally::{self, AccountTally, ModelTally};
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Provider `keyco` (key accounts, openai-chat, model `m1`) whose quota is read from
/// `/keyco/usage`.
fn keyco(mock: &MockUpstream) -> String {
    format!(
        r#"schema = 2
id = "keyco"
category = "apikey"
[auth]
kind = "apikey"
[endpoints.text]
url = "{chat}"
wire = "openai-chat"
retry = {{ 500 = {{ retries = 0 }} }}
[quota]
accounts = "key"
request = {{ url = "{usage}" }}
[[quota.window]]
path = "usage.rolling"
name = "rolling"
unit = "percent"
used = "percent"
[[models]]
id = "m1"
"#,
        chat = mock.url("/keyco/chat/completions"),
        usage = mock.url("/keyco/usage"),
    )
}

async fn keyco_setup() -> Setup {
    setup_signin(|m| vec![("keyco", keyco(m))], &[("keyco", "main"), ("keyco", "spare")], &[], "").await
}

/// These tests read `main`'s history after sending requests that routing would now spread over
/// both accounts (spec 006): `spare` takes no cold work, so every request lands on `main`.
async fn keyco_main_serves() -> Setup {
    let s = keyco_setup().await;
    let path = s._dir.path().join(nullrouter_engine::accounts::FILE);
    let text = std::fs::read_to_string(&path).unwrap().replacen("name = \"spare\"\n", "name = \"spare\"\npriority = 0.0\n", 1);
    nullrouter_engine::files::write_private(&path, &text).unwrap();
    s.engine.reload().await.unwrap();
    s
}

fn chat_ok(prompt: u64, completion: u64) -> Step {
    Step::json(
        200,
        json!({"id": "x", "object": "chat.completion", "model": "m1", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": prompt, "completion_tokens": completion}}),
    )
}

fn usage_ok(percent: f64) -> Step {
    Step::json(200, json!({"usage": {"rolling": {"percent": percent}}}))
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn m1(requests: u64, input: u64, output: u64) -> AccountTally {
    AccountTally::from([("m1".to_owned(), ModelTally { requests, input, output, ..ModelTally::default() })])
}

/// The history of `keyco/main` once every queued poll is written.
async fn kept(s: &Setup) -> Vec<Entry> {
    s.engine.checkpoint_tallies().await;
    history::read(s.engine.home().path(), "keyco", "main", None, None).unwrap()
}

#[tokio::test]
async fn every_poll_is_kept_with_the_tally_since_the_previous_one() {
    let s = keyco_main_serves().await;
    s.mock.on("/keyco/chat", [chat_ok(10, 2), chat_ok(5, 1), chat_ok(3, 3)]);
    s.mock.on("/keyco/usage", [usage_ok(25.0), Step::json(500, json!({"error": "down"})), usage_ok(30.0)]);
    for _ in 0..2 {
        assert!(send(&s, "keyco/m1").await.1.is_ok());
    }
    assert!(s.engine.poll_quota("keyco", "main").await.unwrap().ok());
    let entries = kept(&s).await;
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!((e.v, e.ok), (1, true));
    assert_eq!(e.windows[0].name, "rolling");
    assert_eq!(e.windows[0].used, Some(25.0));
    assert_eq!(e.tally, m1(2, 15, 3), "the first entry carries everything so far");
    assert!(s.engine.history.tally.get("keyco", "main").is_empty(), "taken and reset");

    // A failed poll keeps an entry too, with the tally since the previous entry.
    assert!(send(&s, "keyco/m1").await.1.is_ok());
    assert!(!s.engine.poll_quota("keyco", "main").await.unwrap().ok());
    // No traffic since: an empty tally.
    assert!(s.engine.poll_quota("keyco", "main").await.unwrap().ok());
    let entries = kept(&s).await;
    assert_eq!(entries.len(), 3);
    assert!(!entries[1].ok);
    assert_eq!(entries[1].error.as_ref().unwrap().class, "status");
    assert!(entries[1].windows.is_empty());
    assert_eq!(entries[1].tally, m1(1, 3, 3));
    assert!(entries[2].ok && entries[2].tally.is_empty());

    // The line format and the file modes.
    let home = s.engine.home().path();
    let file = history::history_file(home, "keyco", "main").unwrap();
    let first: Value = serde_json::from_str(std::fs::read_to_string(&file).unwrap().lines().next().unwrap()).unwrap();
    assert_eq!(first["v"], 1);
    assert!(nullrouter_engine::clock::parse_rfc3339(first["at"].as_str().unwrap()).is_some());
    assert_eq!(first["tally"]["m1"]["requests_usage_unreported"], 0);
    assert_eq!(first["tally"]["m1"]["cache_write"], 0);
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(&history::tally_file(home, "keyco", "main").unwrap()), 0o600);
    assert_eq!(mode(&home.join("quota")), 0o700);
    assert_eq!(mode(&home.join("quota/keyco")), 0o700);

    // The tail reader: newest N, oldest first; `since` cuts older ones.
    let last = history::read(home, "keyco", "main", None, Some(2)).unwrap();
    assert_eq!(last, entries[1..]);
    let since = entries[2].time().unwrap();
    let newer = history::read(home, "keyco", "main", Some(since), None).unwrap();
    assert!(!newer.is_empty() && newer.iter().all(|e| e.time().unwrap() >= since));
}

#[tokio::test]
async fn a_graceful_restart_loses_nothing() {
    let s = keyco_main_serves().await;
    s.engine.history.set_checkpoint_every(Duration::from_millis(50));
    s.mock.on("/keyco/chat", [chat_ok(10, 2), chat_ok(4, 4)]);
    let stop = CancellationToken::new();
    let st = stop.clone();
    let upkeep = maintenance::spawn(s.engine.clone(), async move { st.cancelled().await });
    assert!(send(&s, "keyco/m1").await.1.is_ok());
    let home = s.engine.home().path().to_owned();
    let cp = history::tally_file(&home, "keyco", "main").unwrap();
    for _ in 0..200 {
        if cp.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(cp.exists(), "checkpointed on the timer");
    assert_eq!(mode(&cp), 0o600);
    // This one lands after the last timer checkpoint; shutdown writes it.
    s.engine.history.set_checkpoint_every(Duration::from_secs(3600));
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(send(&s, "keyco/m1").await.1.is_ok());
    stop.cancel();
    upkeep.await.unwrap();

    // The maintenance task may have polled meanwhile (a never-polled account is due at
    // once): kept entries plus the reloaded running tally hold every token sent.
    let (again, _) = Engine::open_parity(OperatorHome::new(&home)).unwrap();
    let mut seen = summed(&history::read(&home, "keyco", "main", None, None).unwrap());
    tally::merge(&mut seen, &again.history.tally.get("keyco", "main"));
    assert_eq!(seen, m1(2, 14, 6), "nothing lost across the restart");
    let again = Arc::new(again);
    s.mock.on("/keyco/usage", [usage_ok(10.0)]);
    assert!(again.poll_quota("keyco", "main").await.unwrap().ok());
    again.checkpoint_tallies().await;
    let entries = history::read(&home, "keyco", "main", None, None).unwrap();
    assert_eq!(summed(&entries), m1(2, 14, 6), "the next entry carries the rest");

    // A restart after that entry doesn't count the old checkpoint twice.
    let (third, _) = Engine::open_parity(OperatorHome::new(&home)).unwrap();
    assert!(third.history.tally.get("keyco", "main").is_empty());
}

#[test]
fn a_checkpoint_older_than_the_newest_entry_is_not_reloaded() {
    // A crash between writing an entry and rewriting the checkpoint: the entry already
    // carries the checkpoint's tokens.
    let dir = tempfile::tempdir().unwrap();
    let h = history::History::open(dir.path());
    h.tally.attempt("keyco", "main", "m1", None);
    assert_eq!(h.checkpoint(), 1);
    assert_eq!(history::History::open(dir.path()).tally.get("keyco", "main")["m1"].requests, 1);
    let later = entry_at(SystemTime::now() + Duration::from_secs(5));
    history::append(dir.path(), "keyco", "main", &later).unwrap();
    assert!(history::History::open(dir.path()).tally.get("keyco", "main").is_empty());
}

fn summed(entries: &[Entry]) -> AccountTally {
    let mut out = AccountTally::new();
    for e in entries {
        tally::merge(&mut out, &e.tally);
    }
    out
}

fn entry_at(t: SystemTime) -> Entry {
    let poll = QuotaPoll {
        provider: "keyco".into(),
        account: "main".into(),
        at: t,
        windows: Vec::new(),
        error: None,
        retry: false,
    };
    Entry::from_poll(&poll, m1(1, 1, 1))
}

fn day(n: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_790_000_000 + n * 86_400)
}

#[test]
fn prune_removes_only_older_entries() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    for n in 0..4 {
        history::append(home, "keyco", "main", &entry_at(day(n))).unwrap();
        history::append(home, "keyco", "spare", &entry_at(day(n))).unwrap();
        history::append(home, "other", "main", &entry_at(day(n))).unwrap();
    }
    // Only keyco/main, before day 2.
    assert_eq!(history::prune(home, day(2), Some("keyco"), Some("main")).unwrap(), 2);
    let left: Vec<_> = history::read(home, "keyco", "main", None, None).unwrap().iter().map(|e| e.time()).collect();
    assert_eq!(left, [Some(day(2)), Some(day(3))]);
    assert_eq!(history::read(home, "keyco", "spare", None, None).unwrap().len(), 4);
    let file = history::history_file(home, "keyco", "main").unwrap();
    assert_eq!(mode(&file), 0o600, "rewritten private");
    // Every provider: day 1 and older go everywhere; nothing newer.
    assert_eq!(history::prune(home, day(2), None, None).unwrap(), 4);
    assert_eq!(history::read(home, "other", "main", None, None).unwrap().len(), 2);
    assert_eq!(history::prune(home, day(0), None, None).unwrap(), 0);
}

#[test]
fn forget_deletes_one_accounts_files() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let h = history::History::open(home);
    for a in ["main", "spare"] {
        history::append(home, "keyco", a, &entry_at(day(1))).unwrap();
        h.tally.attempt("keyco", a, "m1", None);
    }
    h.checkpoint();
    assert!(history::forget(home, "keyco", "main").unwrap());
    assert!(!history::history_file(home, "keyco", "main").unwrap().exists());
    assert!(!history::tally_file(home, "keyco", "main").unwrap().exists());
    assert!(history::history_file(home, "keyco", "spare").unwrap().exists());
    assert!(history::tally_file(home, "keyco", "spare").unwrap().exists());
    assert!(!history::forget(home, "keyco", "main").unwrap(), "nothing left");
    assert!(history::forget(home, "../x", "main").is_err(), "names are one path component");
}

#[tokio::test]
async fn removing_an_account_stops_its_polls_but_keeps_its_history() {
    let s = keyco_setup().await;
    s.mock.on("/keyco/usage", [usage_ok(1.0)]);
    assert!(s.engine.poll_quota("keyco", "main").await.unwrap().ok());
    s.engine.checkpoint_tallies().await;
    let home = s.engine.home().path().to_owned();
    let polled =
        |e: &Engine| JobKind::QuotaPoll.due(e, &e.snapshot()).into_iter().map(|(k, _)| k.account).collect::<Vec<_>>();
    assert_eq!(polled(&s.engine), ["main", "spare"]);

    let mut list = Accounts::load(&home.join(accounts::FILE)).unwrap();
    list.remove("keyco", "main").unwrap();
    list.save().unwrap();
    s.engine.reload().await.unwrap();
    assert_eq!(polled(&s.engine), ["spare"], "no more polls");
    assert_eq!(history::read(&home, "keyco", "main", None, None).unwrap().len(), 1, "history kept");
}

#[test]
fn the_tail_reader_reads_across_chunks() {
    let dir = tempfile::tempdir().unwrap();
    for n in 0..300 {
        history::append(dir.path(), "keyco", "main", &entry_at(day(n))).unwrap();
    }
    let last = history::read(dir.path(), "keyco", "main", None, Some(3)).unwrap();
    assert_eq!(last.iter().map(|e| e.time().unwrap()).collect::<Vec<_>>(), [day(297), day(298), day(299)]);
    let since = history::read(dir.path(), "keyco", "main", Some(day(250)), None).unwrap();
    assert_eq!(since.len(), 50);
    assert_eq!(history::read(dir.path(), "keyco", "main", None, None).unwrap().len(), 300);
}
