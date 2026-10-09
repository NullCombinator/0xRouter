//! Spec 011, SC-005: verdicts survive a restart through `routing/verdicts.jsonl`, and return to
//! untested when the account's secret or sign-in, or its plugin, changes (US4 scenarios 5–7).

mod common;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use common::{SECRET, Setup, chat_plugin, setup_file, setup_signin};
use nullrouter_engine::journal::{Journal, Options};
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::MockUpstream;
use nullrouter_engine::verdict::{self, Basis, Board, ComboVerdict, Pair, Rejection, Source, State, Verdict, store};
use nullrouter_registry::OperatorHome;

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

/// `alpha` with key accounts `main` (secret `main`), `spare`, and `env` (secret from the
/// variable `env`); `beta` with `main`.
fn key_accounts(main: &str, env: &str) -> String {
    format!(
        "schema = 1\n\
         [[account]]\nprovider = \"alpha\"\nname = \"main\"\nsecret = \"{main}\"\n\
         [[account]]\nprovider = \"alpha\"\nname = \"spare\"\nsecret = \"{SECRET}-spare\"\n\
         [[account]]\nprovider = \"alpha\"\nname = \"env\"\nsecret = {{ env = \"{env}\" }}\n\
         [[account]]\nprovider = \"beta\"\nname = \"main\"\nsecret = \"{SECRET}-beta\"\n"
    )
}

/// Every variable here is set by cargo for each test process, so no test writes the
/// environment.
const ENV_A: &str = "CARGO_PKG_NAME";
const ENV_B: &str = "CARGO_MANIFEST_DIR";

async fn keyed() -> Setup {
    let plugins =
        |m: &MockUpstream| vec![("alpha", chat_plugin(m, "alpha", "")), ("beta", chat_plugin(m, "beta", ""))];
    setup_file(plugins, &key_accounts(&format!("{SECRET}-main"), ENV_A), "").await
}

/// A PASS on `m1` for each pair, under the pair's basis now.
fn pass_all(s: &Setup, pairs: &[Pair]) {
    let st = s.engine.snapshot();
    for p in pairs {
        let mut v = verdict(State::Pass, 1);
        v.basis = verdict::basis(&s.engine, &st, p);
        s.engine.verdicts.set(p.clone(), v);
    }
}

fn m1(provider: &str, account: &str) -> Pair {
    Pair::new(provider, account, "m1")
}

/// The pairs still with a verdict.
fn kept(s: &Setup) -> Vec<String> {
    s.engine.verdicts.list(&Default::default()).into_iter().map(|(p, _)| p.to_string()).collect()
}

/// `(pair, why)` of every `cleared` line written so far.
async fn cleared(s: &Setup) -> Vec<(String, String)> {
    let engine = s.engine.clone();
    tokio::task::spawn_blocking(move || engine.journal.flush_blocking()).await.unwrap();
    file(s._dir.path())
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|l| l["cleared"] == true)
        .map(|l| (format!("{}/{} {}", l["provider"], l["account"], l["model"]).replace('"', ""), l["why"].to_string()))
        .collect()
}

fn why(pair: &str, why: &str) -> (String, String) {
    (pair.to_owned(), format!("{why:?}"))
}

fn write_accounts(s: &Setup, text: &str) {
    nullrouter_engine::files::write_private(&s._dir.path().join(nullrouter_engine::accounts::FILE), text).unwrap();
}

#[tokio::test]
async fn a_new_key_resets_only_that_accounts_pairs() {
    let s = keyed().await;
    let all = [m1("alpha", "env"), m1("alpha", "main"), m1("alpha", "spare"), m1("beta", "main")];
    pass_all(&s, &all);
    s.engine.reload().await.unwrap();
    assert_eq!(kept(&s).len(), 4, "a reload that changes nothing keeps every verdict");

    write_accounts(&s, &key_accounts(&format!("{SECRET}-main-replaced"), ENV_A));
    s.engine.reload().await.unwrap();
    assert_eq!(kept(&s), ["alpha/env m1", "alpha/spare m1", "beta/main m1"]);
    assert_eq!(cleared(&s).await, [why("alpha/main m1", "account changed")]);
}

#[tokio::test]
async fn a_changed_env_secret_resets_that_account() {
    let s = keyed().await;
    pass_all(&s, &[m1("alpha", "env"), m1("alpha", "main")]);
    // The same account now reads another variable: the value read at load differs.
    write_accounts(&s, &key_accounts(&format!("{SECRET}-main"), ENV_B));
    s.engine.reload().await.unwrap();
    assert_eq!(kept(&s), ["alpha/main m1"]);
    assert_eq!(cleared(&s).await, [why("alpha/env m1", "account changed")]);
}

#[tokio::test]
async fn a_changed_plugin_resets_only_its_providers_pairs() {
    let s = keyed().await;
    pass_all(&s, &[m1("alpha", "main"), m1("alpha", "spare"), m1("beta", "main")]);
    let path = s._dir.path().join("plugins/beta.toml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{text}# updated\n")).unwrap();
    s.engine.reload().await.unwrap();
    assert_eq!(kept(&s), ["alpha/main m1", "alpha/spare m1"]);
    assert_eq!(cleared(&s).await, [why("beta/main m1", "plugin changed")]);
}

#[tokio::test]
async fn a_removed_account_or_provider_drops_its_pairs() {
    let s = keyed().await;
    pass_all(&s, &[m1("alpha", "main"), m1("alpha", "spare"), m1("beta", "main")]);
    let without = |text: String, block: &str| text.replace(block, "");
    let spare = format!("[[account]]\nprovider = \"alpha\"\nname = \"spare\"\nsecret = \"{SECRET}-spare\"\n");
    let beta = format!("[[account]]\nprovider = \"beta\"\nname = \"main\"\nsecret = \"{SECRET}-beta\"\n");
    let text = key_accounts(&format!("{SECRET}-main"), ENV_A);
    write_accounts(&s, &without(without(text, &spare), &beta));
    fs::remove_file(s._dir.path().join("plugins/beta.toml")).unwrap();
    s.engine.reload().await.unwrap();
    assert_eq!(kept(&s), ["alpha/main m1"]);
    assert_eq!(
        cleared(&s).await,
        [why("alpha/spare m1", "account removed"), why("beta/main m1", "provider removed")]
    );
}

#[tokio::test]
async fn a_new_sign_in_resets_the_account_and_a_refresh_does_not() {
    let plugins = |m: &MockUpstream| {
        let signin = format!(
            "[signin]\nflow = \"device_code\"\nclient_id = \"c\"\nrefresh_lead = \"5m\"\n\
             device_url = \"{}\"\ntoken_url = \"{}\"\n",
            m.url("/idp/device"),
            m.url("/idp/token")
        );
        vec![("alpha", format!("{}{signin}", chat_plugin(m, "alpha", "")))]
    };
    let s = setup_signin(plugins, &[("alpha", "a"), ("alpha", "b")], &[("alpha", "a"), ("alpha", "b")], "").await;
    pass_all(&s, &[m1("alpha", "a"), m1("alpha", "b")]);
    let path = nullrouter_engine::tokens::path(s._dir.path());
    let tokens = fs::read_to_string(&path).unwrap();

    // A refresh: a new access token, the same sign-in time.
    let refreshed = tokens.replacen(&format!("{SECRET}-alpha-a"), &format!("{SECRET}-alpha-a-refreshed"), 1);
    nullrouter_engine::files::write_private(&path, &refreshed).unwrap();
    s.engine.reload().await.unwrap();
    assert_eq!(kept(&s), ["alpha/a m1", "alpha/b m1"], "FR-019: a refresh is not a change");

    // A new sign-in of `b`: its `signed_in_at` moves.
    let at = refreshed.rfind("signed_in_at = \"2026-10-01T00:00:00Z\"").unwrap();
    let mut signed_in = refreshed.clone();
    signed_in.replace_range(at.., &refreshed[at..].replacen("2026-10-01T00:00:00Z", "2026-10-07T00:00:00Z", 1));
    nullrouter_engine::files::write_private(&path, &signed_in).unwrap();
    s.engine.reload().await.unwrap();
    assert_eq!(kept(&s), ["alpha/a m1"]);
    assert_eq!(cleared(&s).await, [why("alpha/b m1", "account changed")]);
}

#[tokio::test]
async fn a_change_made_while_stopped_is_caught_at_start() {
    let s = keyed().await;
    pass_all(&s, &[m1("alpha", "main"), m1("alpha", "spare")]);
    let engine = s.engine.clone();
    tokio::task::spawn_blocking(move || engine.journal.flush_blocking()).await.unwrap();
    write_accounts(&s, &key_accounts(&format!("{SECRET}-main-replaced"), ENV_A));

    let home = s._dir.path().to_owned();
    let engine = tokio::task::spawn_blocking(move || Engine::open_parity(OperatorHome::new(home)).unwrap().0)
        .await
        .unwrap();
    let left: Vec<String> =
        engine.verdicts.list(&Default::default()).into_iter().map(|(p, _)| p.to_string()).collect();
    assert_eq!(left, ["alpha/spare m1"]);
}
