//! Records survive a crash (spec 006, US5, T060, FR-037, FR-037a). A child process serves traffic
//! over a mock upstream and is killed with `SIGKILL` at a random point; the parent reads the
//! journal as the CLI does, without a server.
//!
//! The child is this test binary run again with `NR_CRASH_CHILD` set (the `crash_child` test is a
//! no-op otherwise).

mod common;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use common::*;
use nullrouter_engine::journal::records::{self, Filter};
use nullrouter_engine::journal::{Journal, Options, Target};
use nullrouter_engine::testkit::Step;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn body(text: &str) -> Value {
    json!({"model": "alpha/m1", "stream": false, "messages": [{"role": "user", "content": text}]})
}

/// The child: serves requests until it is killed, printing `OK <id>` once a client would have had
/// the whole answer, and keeps one request in flight for good.
#[tokio::test]
async fn crash_child() {
    if std::env::var("NR_CRASH_CHILD").is_err() {
        return;
    }
    let s = Arc::new(setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "a")], "").await);
    s.mock.respond(|r| {
        if String::from_utf8_lossy(&r.body).contains("STALL") {
            Step::StallHeaders { hold: Duration::from_secs(120) }
        } else {
            ok()
        }
    });
    println!("HOME {}", s._dir.path().display());
    {
        let s = s.clone();
        tokio::spawn(async move {
            let req = request(&s, "openai-chat", "alpha/m1", body("STALL"), "ak_stall", CancellationToken::new());
            let _ = s.engine.text(s.engine.snapshot(), req).await;
        });
    }
    let mut workers = Vec::new();
    for w in 0..3 {
        let s = s.clone();
        workers.push(tokio::spawn(async move {
            for i in 0.. {
                let req = request(
                    &s,
                    "openai-chat",
                    "alpha/m1",
                    body(&format!("hello {w} {i}")),
                    &format!("ak_{w}"),
                    CancellationToken::new(),
                );
                let id = req.id.clone();
                if s.engine.text(s.engine.snapshot(), req).await.is_ok() {
                    println!("OK {id}");
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}

/// Runs the child until it has acknowledged `after` requests, then kills it. Returns its home and
/// the ids a client would have received whole.
fn run_and_kill(after: usize) -> (PathBuf, Vec<String>) {
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .args(["--exact", "crash_child", "--nocapture", "--test-threads=1"])
        .env("NR_CRASH_CHILD", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let (mut home, mut acked) = (None, Vec::new());
    while acked.len() < after {
        let Some(Ok(line)) = lines.next() else { panic!("the child ended early") };
        // The harness prints "test crash_child ... " before the child's first line.
        if let Some((_, h)) = line.split_once("HOME ") {
            home = Some(PathBuf::from(h));
        } else if let Some(id) = line.strip_prefix("OK ") {
            acked.push(id.to_owned());
        }
    }
    child.kill().unwrap();
    child.wait().unwrap();
    (home.expect("the child printed its home"), acked)
}

fn varied(seed: usize) -> usize {
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().subsec_nanos() as usize;
    5 + (nanos / 1000 + seed * 7) % 40
}

fn newest_segment(home: &Path) -> PathBuf {
    records::segments(home).pop().expect("a segment").1
}

#[test]
fn a_killed_server_loses_no_finished_record_and_the_open_ones_become_interrupted() {
    for round in 0..3 {
        let (home, acked) = run_and_kill(varied(round));

        // Scenario 1: every record whose client got its last byte is there, whole.
        for id in &acked {
            let r = records::get(&home, id).unwrap_or_else(|| panic!("{id} is missing"));
            assert_eq!(r["outcome"], "succeeded", "{id}");
            assert!(r["served_by"]["account"] == "a" && !r["attempts"].as_array().unwrap().is_empty(), "{id}: {r}");
            assert!(r["decision"].is_object() && r["total_ms"].is_number(), "{id}: {r}");
        }

        // Scenario 2: the request that was in flight has its open and attempt lines, and recovery
        // closes it as interrupted. Nothing stays in progress.
        let open_before =
            records::read(&home, &Filter::default()).into_iter().filter(|r| r["outcome"] == "in_progress").count();
        assert!(open_before >= 1, "the stalled request was in flight");
        let interrupted = records::recover(&home, SystemTime::now()).unwrap();
        assert_eq!(interrupted, open_before);
        let all = records::read(&home, &Filter::default());
        assert!(all.iter().all(|r| r["outcome"] != "in_progress"));
        let stalled: Vec<_> = all.iter().filter(|r| r["outcome"] == "interrupted").collect();
        assert_eq!(stalled.len(), open_before);
        for r in &stalled {
            assert!(r["recovered_at"].is_string());
            // A request killed before its first update has only the open line the store wrote.
            assert_eq!(r["style"], "openai-chat", "its open line survived: {r}");
        }
        assert!(stalled.iter().any(|r| r["target"] == "alpha/m1"), "the stalled request kept its target");
        assert!(
            stalled.iter().any(|r| !r["attempts"].as_array().unwrap().is_empty()),
            "the stalled request's attempt line survived"
        );
        for id in &acked {
            assert_eq!(
                records::get(&home, id).unwrap()["outcome"],
                "succeeded",
                "recovery leaves finished records alone"
            );
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}

#[tokio::test]
async fn a_torn_final_line_is_cut_and_the_next_line_is_whole() {
    let (home, _) = tokio::task::spawn_blocking(|| run_and_kill(8)).await.unwrap();
    let segment = newest_segment(&home);
    let whole = std::fs::read_to_string(&segment).unwrap();
    // The kill may have left a torn line already; make sure there is one.
    let mut torn = whole.clone();
    if torn.ends_with('\n') {
        torn.push_str("{\"v\":1,\"t\":\"attempt\",\"id\":\"rq_x\",\"attem");
    }
    std::fs::write(&segment, &torn).unwrap();

    records::recover(&home, SystemTime::now()).unwrap();
    let journal = Journal::start(&home, Options::default()).unwrap();
    journal.append(
        Target::Records { day: segment.file_stem().unwrap().to_str().unwrap().into() },
        "open",
        json!({"id": "rq_next"}),
    );
    journal.flush_blocking();
    let text = std::fs::read_to_string(&segment).unwrap();
    assert!(text.ends_with('\n'));
    for (n, line) in text.lines().enumerate() {
        assert!(serde_json::from_str::<Value>(line).is_ok(), "line {n} is whole: {line:?}");
    }
    assert!(text.contains("rq_next"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn a_power_loss_keeps_everything_older_than_the_sync_interval_across_a_new_day() {
    let home = tempfile::tempdir().unwrap();
    let sync = Duration::from_millis(40);
    let journal =
        Journal::start(home.path(), Options { sync_every: sync, retry_every: Duration::from_secs(60) }).unwrap();
    let started = std::time::Instant::now();
    let mut written = Vec::new();
    for i in 0..60 {
        // The day changes halfway: a new segment starts mid-run.
        let day = if i < 30 { "2026-10-04" } else { "2026-10-05" };
        journal.append(Target::Records { day: day.into() }, "open", json!({"id": format!("rq_{i:03}")}));
        written.push((i, started.elapsed()));
        std::thread::sleep(Duration::from_millis(5));
    }
    // The instant of the power loss: whatever the files hold past their last `fdatasync` is gone.
    let lost_at = started.elapsed();
    let synced = journal.faults().synced.lock().unwrap().clone();
    drop(journal);
    for (path, len) in &synced {
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_len((*len).min(f.metadata().unwrap().len())).unwrap();
    }
    // Files that never got a sync are what a power loss before any data sync leaves: empty.
    for (_, path) in records::segments(home.path()) {
        if !synced.contains_key(&path) {
            std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(0).unwrap();
        }
    }

    let kept: std::collections::BTreeSet<String> =
        records::read(home.path(), &Filter::default()).iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect();
    for (i, at) in &written {
        // `open` lines alone fold into records; anything older than a few sync rounds must be there.
        if lost_at.saturating_sub(*at) > sync * 6 {
            assert!(
                kept.contains(&format!("rq_{i:03}")),
                "rq_{i:03} written {:?} before the loss is missing",
                lost_at - *at
            );
        }
    }
    assert!(kept.len() >= 20, "most of the run is there: {}", kept.len());
    assert_eq!(records::segments(home.path()).len(), 2, "both days' segments still exist");
}

#[tokio::test]
async fn when_the_disk_refuses_writes_clients_still_get_answers_and_writing_resumes() {
    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "a")], "").await;
    s.mock.respond(|_| ok());
    let send_one = |i: usize| {
        let s = &s;
        async move {
            let req = request(s, "openai-chat", "alpha/m1", body(&format!("hi {i}")), "ak_x", CancellationToken::new());
            let id = req.id.clone();
            s.engine.text(s.engine.snapshot(), req).await.expect("the client still gets its answer");
            settled(s, &id).await;
            id
        }
    };
    let before = send_one(0).await;

    s.engine.journal.faults().fail_writes.store(true, std::sync::atomic::Ordering::Relaxed);
    let during = [send_one(1).await, send_one(2).await];
    let h = s.engine.journal.health();
    assert!(!h.kept && h.since.is_some() && h.held_lines > 0, "{h:?}");

    s.engine.journal.faults().fail_writes.store(false, std::sync::atomic::Ordering::Relaxed);
    let mut resumed = false;
    for _ in 0..300 {
        if s.engine.journal.health().kept {
            resumed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(resumed, "writing resumed without a restart");
    let after = send_one(3).await;
    let engine = s.engine.clone();
    tokio::task::spawn_blocking(move || engine.journal.flush_blocking()).await.unwrap();

    for id in std::iter::once(&before).chain(&during).chain(std::iter::once(&after)) {
        let r = records::get(s._dir.path(), id).unwrap_or_else(|| panic!("{id} reached the disk"));
        assert_eq!(r["outcome"], "succeeded", "{id}");
    }
}

fn timed_attempt(timing: Option<nullrouter_engine::records::AttemptTiming>) -> nullrouter_engine::records::Attempt {
    use nullrouter_engine::records::{Attempt, AttemptKind, AttemptOutcome};
    Attempt {
        n: 1,
        provider: "alpha".into(),
        account: Some("a".into()),
        model: "m1".into(),
        kind: AttemptKind::Initial,
        started: 1.0,
        ended: Some(90.0),
        outcome: Some(AttemptOutcome::Ok),
        usage: None,
        dropped: Vec::new(),
        forced: Vec::new(),
        placement: None,
        adapter: None,
        member: None,
        timing,
    }
}

/// Journals one finished request holding `attempt` and reads it back as the CLI does.
fn journal_one(id: &str, attempt: nullrouter_engine::records::Attempt) -> Value {
    use nullrouter_engine::records::{Outcome, RecordStore, RequestRecord};
    let home = tempfile::tempdir().unwrap();
    let journal = Arc::new(Journal::start(home.path(), Options::default()).unwrap());
    let store = RecordStore::journaled(journal.clone());
    store.insert(RequestRecord::new(id.into(), "2026-10-07T10:00:00Z".into(), "openai-chat"));
    store.update(id, |r| {
        r.attempts.push(attempt);
        r.outcome = Outcome::Succeeded;
        r.total_ms = Some(90.0);
    });
    journal.flush_blocking();
    let read = records::read(home.path(), &Filter::default());
    assert_eq!(read.len(), 1);
    read[0].clone()
}

#[test]
fn a_record_from_before_the_slice_has_no_timing() {
    let r = journal_one("rq_old", timed_attempt(None));
    assert!(r["attempts"][0].get("timing").is_none_or(Value::is_null), "{r}");
}

#[test]
fn a_record_with_timing_round_trips_through_the_journal() {
    use nullrouter_engine::records::{AttemptTiming, Connection};
    let t = AttemptTiming {
        retry_wait_ms: Some(200.0),
        connected: Some(12.5),
        connection: Connection::New,
        http: Some("2".into()),
        proxy: Some("eu-exit".into()),
        headers: Some(40.25),
        first_output: Some(55.5),
        upstream_done: Some(80.0),
        blocked_ms: 3.5,
        ..AttemptTiming::default()
    };
    let r = journal_one("rq_new", timed_attempt(Some(t.clone())));
    assert_eq!(r["attempts"][0]["timing"], serde_json::to_value(&t).unwrap());
    assert_eq!(r["attempts"][0]["timing"]["connection"], "new");
}
