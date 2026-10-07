//! Spec 011, SC-005 first half: verdicts survive a restart through `routing/verdicts.jsonl`.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use nullrouter_engine::journal::{Journal, Options};
use nullrouter_engine::verdict::{Basis, Board, ComboVerdict, Pair, Rejection, Source, State, Verdict, store};

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000 + secs)
}

fn verdict(state: State, n: u64) -> Verdict {
    Verdict {
        state,
        reason: format!("reason {n}"),
        rejection: (state == State::Broken).then_some(Rejection::ModelNotAvailable),
        source: Source::Test,
        at: at(n),
        record: Some(format!("rq_{n}")),
        step: (state == State::Unknown).then_some(0),
        next: (state == State::Unknown).then(|| at(n + 60)),
        basis: Basis { secret: Some("sha256:ab".into()), signed_in_at: None, plugin: "sha256:cd".into() },
        note: None,
    }
}

fn open(home: &Path) -> (Arc<Journal>, Board, store::Replay) {
    let journal = Arc::new(Journal::start(home, Options::default()).unwrap());
    let (board, replay) = Board::open(home, journal.clone());
    (journal, board, replay)
}

fn file(home: &Path) -> String {
    fs::read_to_string(home.join("routing/verdicts.jsonl")).unwrap_or_default()
}

#[test]
fn set_clear_and_replay_after_restart() {
    let home = tempfile::tempdir().unwrap();
    let a = Pair::new("anthropic", "max", "claude-opus-4-1");
    let b = Pair::new("openrouter", "main", "anthropic/claude-sonnet-4.5");
    let c = Pair::new("ollama", "-", "llama3");
    {
        let (journal, board, replay) = open(home.path());
        assert_eq!((replay.lines, replay.verdicts.len()), (0, 0));
        board.set(a.clone(), verdict(State::Broken, 1));
        board.set(b.clone(), verdict(State::Unknown, 2));
        board.set(c.clone(), verdict(State::Pass, 3));
        board.set(b.clone(), verdict(State::Pass, 4));
        assert!(board.clear(&c, "cleared by the operator", at(5)));
        assert!(!board.clear(&c, "cleared by the operator", at(6)), "an untested pair has nothing to clear");
        assert!(board.is_broken("anthropic", "max", "claude-opus-4-1"));
        assert!(!board.is_broken("anthropic", "other", "claude-opus-4-1"), "a verdict is per account");
        journal.flush_blocking();
    }
    let text = file(home.path());
    assert_eq!(text.lines().count(), 5, "{text}");
    assert!(text.lines().last().unwrap().contains("\"cleared\":true"), "{text}");
    assert!(text.contains("\"rejection\":\"model_not_available\""), "{text}");

    let (_journal, board, replay) = open(home.path());
    assert_eq!((replay.lines, replay.malformed, replay.verdicts.len()), (5, 0, 2));
    assert_eq!(board.get(&a), Some(verdict(State::Broken, 1)));
    assert_eq!(board.get(&b), Some(verdict(State::Pass, 4)), "the last line per pair wins");
    assert_eq!(board.get(&c), None);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(home.path().join("routing/verdicts.jsonl")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

/// A writer killed mid-append leaves a torn last line: replay skips it and keeps the rest.
#[test]
fn a_torn_last_line_loses_only_that_line() {
    let home = tempfile::tempdir().unwrap();
    let pairs: Vec<Pair> = (0..3).map(|i| Pair::new("p", "a", format!("m{i}"))).collect();
    {
        let (journal, board, _) = open(home.path());
        for (i, p) in pairs.iter().enumerate() {
            board.set(p.clone(), verdict(State::Pass, i as u64));
        }
        journal.flush_blocking();
    }
    let path = home.path().join("routing/verdicts.jsonl");
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    f.write_all(b"{\"v\":1,\"t\":\"verdict\",\"provider\":\"p\",\"account\":\"a\",\"model\":\"m9\",\"sta").unwrap();
    drop(f);

    let (_journal, board, replay) = open(home.path());
    assert_eq!((replay.lines, replay.malformed), (4, 1));
    for p in &pairs {
        assert_eq!(board.get(p).map(|v| v.state), Some(State::Pass), "{p}");
    }
    assert_eq!(board.get(&Pair::new("p", "a", "m9")), None);
}

/// Compaction keeps every live pair and combo result, and drops the history behind them.
#[test]
fn compaction_keeps_every_live_entry() {
    let home = tempfile::tempdir().unwrap();
    let live: Vec<Pair> = (0..5).map(|i| Pair::new("p", "a", format!("m{i}"))).collect();
    {
        let (journal, board, _) = open(home.path());
        for round in 0..30u64 {
            for p in &live {
                board.set(p.clone(), verdict(if round % 2 == 0 { State::Unknown } else { State::Pass }, round));
            }
        }
        board.set_combo(
            "coder",
            ComboVerdict {
                state: State::Pass,
                answered_by: Some("glm".into()),
                reason: String::new(),
                at: at(99),
                record: Some("rq_99".into()),
                step: None,
                next: None,
                definition: "sha256:ef".into(),
            },
        );
        journal.flush_blocking();
    }
    let lines = file(home.path()).lines().count();
    assert!(lines < 150, "compacted: {lines} lines for 151 changes");

    let (_journal, board, replay) = open(home.path());
    assert_eq!(replay.malformed, 0);
    assert_eq!(replay.verdicts.len(), 5);
    for p in &live {
        assert_eq!(board.get(p), Some(verdict(State::Pass, 29)), "{p}");
    }
    assert_eq!(board.combo("coder").and_then(|c| c.answered_by), Some("glm".into()));
}
