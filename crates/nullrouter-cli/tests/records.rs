//! `nullrouter records` reads the journal from disk (spec 006, US5, T062; contracts/operator-cli.md):
//! `list` and `show` without a server and with every filter, `in progress` against `interrupted`,
//! `prune --before`, and `forget` for an account or an agent.

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use nullrouter_engine::journal::records::{self, Who};
use nullrouter_engine::journal::state;
use serde_json::{Value, json};

fn nr(home: &Path, args: &[&str]) -> Output {
    nr_env(home, args, &[])
}

fn nr_env(home: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn text(o: &Output) -> String {
    format!("{}{}", out(o), String::from_utf8_lossy(&o.stderr))
}

fn ok(home: &Path, args: &[&str]) -> String {
    let o = nr(home, args);
    assert!(o.status.success(), "{args:?}: {}", text(&o));
    out(&o)
}

fn segment(home: &Path, day: &str, lines: &[Value]) {
    std::fs::create_dir_all(home.join("records")).unwrap();
    let mut body = String::new();
    for l in lines {
        body += &(l.to_string() + "\n");
    }
    let path = home.join(format!("records/{day}.jsonl"));
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(path, old + &body).unwrap();
}

fn open(id: &str, at: &str, agent: &str, target: &str) -> Value {
    json!({"v":1,"t":"open","id":id,"arrived":at,"agent":agent,"style":"anthropic-messages","op":"generate","type":"text","target":target})
}

fn attempt(id: &str, provider: &str, account: &str, reason: &str) -> Value {
    json!({"v":1,"t":"attempt","id":id,"attempt":{"n":1,"provider":provider,"account":account,"model":"claude-sonnet-4-5","kind":"initial",
        "placement":{"reason":reason,"rank":0},"started":0.4,"ended":900.0,"outcome":{"state":"ok"},"dropped":[],"forced":[]}})
}

fn close(id: &str, provider: &str, account: &str) -> Value {
    json!({"v":1,"t":"close","id":id,"outcome":"succeeded","served_by":{"provider":provider,"account":account,"model":"claude-sonnet-4-5"},
        "ttft_ms":120.0,"total_ms":900.0,"usage":null,"break_handling":{"kind":"none"},"job":null})
}

/// A finished request: (id, arrival, agent, target), served by (provider, account) for `reason`.
fn done(home: &Path, day: &str, req: (&str, &str, &str, &str), by: (&str, &str), reason: &str) {
    let ((id, at, agent, target), (provider, account)) = (req, by);
    segment(
        home,
        day,
        &[open(id, at, agent, target), attempt(id, provider, account, reason), close(id, provider, account)],
    );
}

fn ids(o: &str) -> Vec<String> {
    o.lines().filter_map(|l| l.split_whitespace().next()).filter(|w| w.starts_with("rq_")).map(str::to_owned).collect()
}

fn seeded() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    done(
        h,
        "2026-10-02",
        ("rq_01", "2026-10-02T09:00:00Z", "key_a", "sonnet"),
        ("anthropic", "max"),
        "cold_by_deficit",
    );
    done(h, "2026-10-03", ("rq_02", "2026-10-03T09:00:00Z", "key_b", "sonnet"), ("anthropic", "pro"), "warm");
    done(h, "2026-10-04", ("rq_03", "2026-10-04T09:00:00Z", "key_a", "opus"), ("openrouter", "main"), "payg_overflow");
    done(h, "2026-10-04", ("rq_04", "2026-10-04T10:00:00Z", "key_a", "sonnet"), ("anthropic", "max"), "warm");
    dir
}

#[test]
fn list_and_show_read_the_journal_without_a_server_and_every_filter_narrows() {
    let dir = seeded();
    let h = dir.path();
    let all = ok(h, &["records", "list"]);
    assert_eq!(ids(&all), ["rq_04", "rq_03", "rq_02", "rq_01"], "newest first:\n{all}");

    for (args, want) in [
        (vec!["--provider", "openrouter"], vec!["rq_03"]),
        (vec!["--account", "anthropic/max"], vec!["rq_04", "rq_01"]),
        (vec!["--agent", "key_b"], vec!["rq_02"]),
        (vec!["--model", "opus"], vec!["rq_03"]),
        (vec!["--reason", "warm"], vec!["rq_04", "rq_02"]),
        (vec!["--since", "2026-10-03"], vec!["rq_04", "rq_03", "rq_02"]),
        (vec!["--since", "2026-10-04T09:30:00Z"], vec!["rq_04"]),
        (vec!["--limit", "2"], vec!["rq_04", "rq_03"]),
        (vec!["--agent", "key_a", "--reason", "warm"], vec!["rq_04"]),
    ] {
        let mut a = vec!["records", "list"];
        a.extend(&args);
        assert_eq!(ids(&ok(h, &a)), want, "{args:?}");
    }

    let json: Value = serde_json::from_str(&ok(h, &["--json", "records", "list", "--limit", "1"])).unwrap();
    assert_eq!(json[0]["id"], "rq_04");
    assert_eq!(json[0]["served_by"]["account"], "max");
    assert_eq!(json[0]["attempts"][0]["placement"]["reason"], "warm");

    let shown = ok(h, &["records", "show", "rq_03"]);
    assert!(shown.contains("rq_03") && shown.contains("openrouter/main"), "{shown}");
    let one: Value = serde_json::from_str(&ok(h, &["--json", "records", "show", "rq_03"])).unwrap();
    assert_eq!(one["target"], "opus");
    assert_eq!(nr(h, &["records", "show", "rq_missing"]).status.code(), Some(1));
    assert_eq!(nr(h, &["records", "list", "--since", "yesterday"]).status.code(), Some(1));
}

#[tokio::test]
async fn an_open_without_a_close_is_in_progress_with_a_server_and_interrupted_without() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    let serving = serve(h).await;
    // A request still in flight: the server's writer owns the file, the open line is all there is.
    let today = nullrouter_engine::clock::now_rfc3339();
    segment(h, &today[..10], &[open("rq_live", &today, "key_a", "sonnet")]);
    let with = ok(h, &["records", "list"]);
    assert!(with.contains("rq_live") && with.contains("in progress"), "{with}");
    let shown: Value = serde_json::from_str(&ok(h, &["--json", "records", "show", "rq_live"])).unwrap();
    assert_eq!(shown["outcome"], "in_progress");

    drop(serving);
    let without = ok(h, &["records", "list"]);
    assert!(without.contains("rq_live") && without.contains("interrupted"), "{without}");
}

#[test]
fn prune_removes_only_older_records_and_says_how_many() {
    let dir = seeded();
    let h = dir.path();
    let said = ok(h, &["records", "prune", "--before", "2026-10-03"]);
    assert!(said.contains("pruned 1 records"), "{said}");
    assert_eq!(ids(&ok(h, &["records", "list"])), ["rq_04", "rq_03", "rq_02"]);
    assert!(!h.join("records/2026-10-02.jsonl").exists());
    let again = ok(h, &["records", "prune", "--before", "2026-10-03"]);
    assert!(again.contains("pruned 0 records"), "{again}");
    assert_eq!(nr(h, &["records", "prune", "--before", "soon"]).status.code(), Some(1));
}

#[test]
fn forget_removes_an_accounts_or_an_agents_records_and_the_agents_fingerprints() {
    let dir = seeded();
    let h = dir.path();
    // key_a holds a fingerprint; so does key_b.
    std::fs::create_dir_all(h.join("routing")).unwrap();
    let warm = |agent: &str| {
        json!({"v":1,"t":"warm","agent":agent,"hash":"00112233445566778899aabbccddeeff","provider":"anthropic","account":"max",
            "model":"claude-sonnet-4-5","prefix_tokens":100,"last_used":"2026-10-04T10:00:00Z"})
        .to_string()
    };
    std::fs::write(h.join("routing/warm.jsonl"), format!("{}\n{}\n", warm("key_a"), warm("key_b"))).unwrap();

    let said = ok(h, &["records", "forget", "--account", "anthropic/pro"]);
    assert!(said.contains("forgot 1 records"), "{said}");
    assert_eq!(ids(&ok(h, &["records", "list"])), ["rq_04", "rq_03", "rq_01"]);

    let said = ok(h, &["records", "forget", "--agent", "key_a"]);
    assert!(said.contains("forgot 3 records"), "{said}");
    assert!(ids(&ok(h, &["records", "list"])).is_empty());
    let left = state::load(h).warm;
    assert_eq!(left.len(), 1, "key_a's fingerprint went with its records");
    assert_eq!(left[0].agent, "key_b");

    assert_eq!(nr(h, &["records", "forget"]).status.code(), Some(2), "one of --account or --agent is required");
    assert_eq!(nr(h, &["records", "forget", "--account", "max"]).status.code(), Some(1), "provider/name");
}

#[test]
fn a_busy_journal_lock_is_exit_1() {
    let dir = seeded();
    let h = dir.path();
    let held = records::lock(h, Duration::from_secs(1)).unwrap();
    let env = [("NULLROUTER_JOURNAL_LOCK_WAIT_MS", "150")];
    for args in [vec!["records", "prune", "--before", "2026-10-03"], vec!["records", "forget", "--agent", "key_a"]] {
        let o = nr_env(h, &args, &env);
        assert_eq!(o.status.code(), Some(1), "{args:?}: {}", text(&o));
        assert!(text(&o).contains("busy"), "{}", text(&o));
    }
    assert_eq!(ids(&ok(h, &["records", "list"])).len(), 4, "nothing was touched");
    drop(held);
    assert!(nr_env(h, &["records", "prune", "--before", "2026-10-03"], &env).status.success());
    let _ = Who::Agent(String::new());
}

// ---- a running server ----

struct Serving(Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn serve(home: &Path) -> Serving {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let listen = format!("127.0.0.1:{port}");
    let child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(["serve", "--listen", &listen])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let serving = Serving(child);
    for _ in 0..500 {
        if home.join("run/operator.sock").exists() && tokio::net::TcpStream::connect(&listen).await.is_ok() {
            return serving;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the server never came up");
}

#[test]
fn a_clients_escape_sequences_never_reach_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    segment(
        h,
        "2026-10-04",
        &[
            json!({"v":1,"t":"open","id":"rq_01","arrived":"2026-10-04T09:00:00Z","agent":"key_a","session":"s\u{1b}[31mred",
                "style":"anthropic-messages","op":"generate","type":"text","target":"evil\u{1b}]0;owned\u{7}"}),
            attempt("rq_01", "anthropic", "max", "warm"),
        ],
    );
    for args in [vec!["records", "list"], vec!["records", "show", "rq_01"]] {
        let o = nr(h, &args);
        assert!(o.status.success(), "{args:?}: {}", text(&o));
        assert!(!out(&o).chars().any(|c| c.is_control() && c != '\n'), "{args:?}: {:?}", out(&o));
        assert!(out(&o).contains("evil]0;owned"), "{args:?}: {}", out(&o));
    }
    // JSON keeps the text exactly; its escapes are printable.
    let o = nr(h, &["--json", "records", "show", "rq_01"]);
    assert!(out(&o).contains("\\u001b"), "{}", out(&o));
}

#[test]
fn forget_says_so_when_the_routing_state_could_not_be_rewritten() {
    let dir = seeded();
    let h = dir.path();
    std::fs::create_dir_all(h.join("routing")).unwrap();
    let warm = json!({"v":1,"t":"warm","agent":"key_a","hash":"00112233445566778899aabbccddeeff","provider":"anthropic",
        "account":"max","model":"claude-sonnet-4-5","prefix_tokens":100,"last_used":"2026-10-04T10:00:00Z"});
    std::fs::write(h.join("routing/warm.jsonl"), warm.to_string() + "\n").unwrap();
    // The replacement file can't be written: its temporary name is a directory.
    std::fs::create_dir(h.join("routing/warm.jsonl.tmp")).unwrap();
    let o = nr(h, &["records", "forget", "--agent", "key_a"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("routing state could not be rewritten"), "{}", text(&o));
    assert_eq!(state::load(h).warm.len(), 1, "the fingerprint is still there, and the command said so");
}
