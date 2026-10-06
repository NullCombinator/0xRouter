//! Fixture operator homes (spec 008, research R4), shared by the CLI's golden suite and the
//! server's two-route tests. Each is written file by file in the formats the engine and the
//! registry load, with fixed ids and times around [`NOW`]; nothing reads the real clock or the
//! environment. Every file is loaded back through its own loader before the home is returned,
//! so a stale fixture fails here and not in a golden.

use std::fs;
use std::path::Path;

use tempfile::TempDir;
use ulid::Ulid;

use crate::accounts::{self, Accounts};
use crate::clock::parse_rfc3339;
use crate::keys::{self, Keys};
use crate::tokens::{self, TokenStore};

/// The pinned time the goldens run at (`NULLROUTER_TEST_NOW`), a Saturday.
pub const NOW: &str = "2026-10-03T14:23:00Z";

/// A fixture: its name (a golden directory) and whether a server may run on it. A home with a
/// polled account is stopped-only, because a running server would poll that account's provider.
pub struct Fixture {
    pub name: &'static str,
    pub build: fn() -> TempDir,
    pub can_serve: bool,
}

pub const ALL: &[Fixture] = &[
    Fixture { name: "empty", build: empty, can_serve: true },
    Fixture { name: "full", build: full, can_serve: true },
    Fixture { name: "polled", build: polled, can_serve: false },
    Fixture { name: "broken", build: broken, can_serve: false },
];

fn put(home: &Path, rel: &str, text: &str) {
    let path = home.join(rel);
    // Directories and files are private, as the server makes them (`check` warns about others).
    let mut dirs = std::fs::DirBuilder::new();
    dirs.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut dirs, 0o700);
    dirs.create(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

/// A home with no files at all.
pub fn empty() -> TempDir {
    tempfile::tempdir().unwrap()
}

/// A home whose `config.toml` doesn't parse, for the startup-error path.
pub fn broken() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "config.toml", "schema = 1\n[[unified_model]\nname = \"x\"\n");
    dir
}

/// Every account kind and state that needs no network, unified models (one whose members differ
/// in `context_length`, one dropped because a member's plugin was skipped), bundled, installed
/// and invalid plugins, keys, quota history, and records over several days.
pub fn full() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();

    put(
        h,
        accounts::FILE,
        r#"schema = 2

[[account]]
provider = "anthropic"
name = "main"
secret = "sk-fixture-main-AAAA0001"
order = 1

[[account]]
provider = "anthropic"
name = "spare"
secret = "sk-fixture-spare-BBBB0002"
order = 2
disabled = true

[[account]]
provider = "openrouter"
name = "envkey"
secret = { env = "NR_FIXTURE_UNSET_VAR" }
order = 3
priority = 0.5

[[account]]
provider = "xai"
name = "work"
kind = "signin"
order = 4

[[account]]
provider = "xai"
name = "old"
kind = "signin"
order = 5

[[account]]
provider = "xai"
name = "gone"
kind = "signin"
order = 6
"#,
    );
    put(
        h,
        tokens::FILE,
        r#"schema = 1

[[token]]
provider = "xai"
name = "work"
access_token = "xai-access-fixture-WORK1111"
refresh_token = "xai-refresh-fixture-WORK2222"
expires_at = "2026-10-03T20:23:00Z"
scope = "openid profile"
hosts = ["auth.x.ai"]
signed_in_at = "2026-10-01T09:00:00Z"
last_refresh_at = "2026-10-03T12:00:00Z"
[token.claims]
email = "work@example.test"
tier = "pro"

[[token]]
provider = "xai"
name = "old"
access_token = "xai-access-fixture-OLD33333"
expires_at = "2026-10-02T09:00:00Z"
hosts = ["auth.x.ai"]
signed_in_at = "2026-09-20T09:00:00Z"
state = "needs_sign_in"
state_since = "2026-10-02T09:05:00Z"
state_reason = "the refresh token expired"

[[token]]
provider = "xai"
name = "gone"
access_token = "xai-access-fixture-GONE4444"
expires_at = "2026-10-02T10:00:00Z"
hosts = ["auth.x.ai"]
signed_in_at = "2026-09-21T09:00:00Z"
state = "refused"
state_since = "2026-10-02T10:05:00Z"
state_reason = "invalid_grant"
"#,
    );
    put(
        h,
        keys::FILE,
        r#"schema = 1

[[key]]
id = "ak_fixture1"
name = "laptop"
digest = "0000000000000000000000000000000000000000000000000000000000000001"
last4 = "L0P1"
created = "2026-10-01T08:00:00Z"

[[key]]
id = "ak_fixture2"
name = "ci"
digest = "0000000000000000000000000000000000000000000000000000000000000002"
last4 = "C1C1"
created = "2026-10-01T08:30:00Z"
revoked = "2026-10-02T17:00:00Z"
break_behaviour = "error_event"
"#,
    );

    // Plugins: bluesminds installed from the community set (its limits differ from grok-cli's),
    // and one invalid user plugin whose unified model is therefore dropped.
    let community = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/community/bluesminds.toml");
    put(h, "plugins/bluesminds.toml", &fs::read_to_string(community).unwrap());
    put(h, "plugins/broken.toml", "schema = 2\nid = \"broken\"\nthis is not valid\n");
    put(
        h,
        "config.toml",
        r#"schema = 1

[[unified_model]]
name = "mixed"
members = [
  { provider = "grok-cli", model = "grok-build" },
  { provider = "bluesminds", model = "claude-sonnet-4-5" },
]

[[unified_model]]
name = "lost"
members = [
  { provider = "broken", model = "m1" },
  { provider = "xai", model = "grok-4" },
]
"#,
    );

    // Quota history for xai/work: a good poll with traffic, then a failed one.
    put(
        h,
        "quota/xai/work.jsonl",
        concat!(
            r#"{"v":1,"at":"2026-10-03T14:00:00.000Z","ok":true,"windows":[{"name":"monthly","unit":"credits","used":1240.0,"limit":5000.0,"remaining":3760.0,"resets_at":"2026-11-01T00:00:00.000Z"}],"tally":{"grok-4":{"requests":12,"requests_usage_unreported":1,"input":5120,"output":2210,"cache_read":40960,"cache_write":1024}}}"#,
            "\n",
            r#"{"v":1,"at":"2026-10-03T14:10:00.000Z","ok":false,"error":{"class":"timeout","reason":"no answer within 30 s"},"windows":[],"tally":{}}"#,
            "\n",
        ),
    );

    records(h);
    check_loads(h);
    dir
}

/// A signed-in account of a provider that polls quota, with its history. Stopped-only.
pub fn polled() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    put(h, accounts::FILE, "schema = 2\n\n[[account]]\nprovider = \"grok-cli\"\nname = \"work\"\nkind = \"signin\"\n");
    put(
        h,
        tokens::FILE,
        r#"schema = 1

[[token]]
provider = "grok-cli"
name = "work"
access_token = "grok-access-fixture-5555"
expires_at = "2026-10-03T20:23:00Z"
hosts = ["auth.grok.com"]
signed_in_at = "2026-10-01T09:00:00Z"
"#,
    );
    put(
        h,
        "quota/grok-cli/work.jsonl",
        concat!(
            r#"{"v":1,"at":"2026-10-03T14:18:00.000Z","ok":true,"windows":[{"name":"prepaid","unit":"credits","used":1240.0,"limit":5000.0,"remaining":3760.0,"resets_at":"2026-11-01T00:00:00.000Z"}],"tally":{"grok-build":{"requests":3,"requests_usage_unreported":0,"input":900,"output":300,"cache_read":0,"cache_write":0}}}"#,
            "\n"
        ),
    );
    check_loads(h);
    dir
}

/// `rq_` + a ULID made at `at`, so ids sort in arrival order like the engine's.
fn id(at: &str, n: u64) -> String {
    let ms = parse_rfc3339(at).unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    format!("rq_{}", Ulid::from_parts(ms, u128::from(n)))
}

/// The id of the fixture record numbered `n` (1 to 6, in arrival order): 4 and 1 finished with
/// usage, 2 and 4 without, 5 cut short or in flight (an attempt behind it), 6 only an `open`.
pub fn record_id(n: u64) -> String {
    let at = [
        "2026-10-01T09:00:00Z",
        "2026-10-02T09:00:00Z",
        "2026-10-02T11:30:00Z",
        "2026-10-03T09:00:00Z",
        "2026-10-03T13:00:00Z",
        "2026-10-03T14:22:00Z",
    ];
    id(at[n as usize - 1], n)
}

fn line(v: serde_json::Value) -> String {
    v.to_string() + "\n"
}

fn open(id: &str, at: &str, agent: &str, target: &str) -> String {
    line(
        serde_json::json!({"v":1,"t":"open","id":id,"arrived":at,"agent":agent,"style":"anthropic-messages","op":"generate","type":"text","target":target}),
    )
}

fn attempt(id: &str, provider: &str, account: &str, model: &str, reason: &str) -> String {
    line(
        serde_json::json!({"v":1,"t":"attempt","id":id,"attempt":{"n":1,"provider":provider,"account":account,"model":model,"kind":"initial",
        "placement":{"reason":reason,"rank":0},"started":0.4,"ended":900.0,"outcome":{"state":"ok"},"dropped":[],"forced":[]}}),
    )
}

fn close(id: &str, provider: &str, account: &str, model: &str, usage: serde_json::Value) -> String {
    line(
        serde_json::json!({"v":1,"t":"close","id":id,"outcome":"succeeded","served_by":{"provider":provider,"account":account,"model":model},
        "ttft_ms":120.0,"total_ms":900.0,"usage":usage,"break_handling":{"kind":"none"},"job":null}),
    )
}

/// Records over three days: finished with usage, finished with usage not reported, one by a key
/// `keys.toml` names, and one by an unnamed agent. See [`unfinished`] for the rest.
fn records(h: &Path) {
    let usage = serde_json::json!({"input":1204,"output":388,"cache_read":18432,"cache_write":null,"estimated":false});
    let seg = |day: &str, text: String| {
        let path = format!("records/{day}.jsonl");
        let old = fs::read_to_string(h.join(&path)).unwrap_or_default();
        put(h, &path, &(old + &text));
    };
    let finished = |day: &str, at: &str, n: u64, agent: &str, target: &str, by: (&str, &str, &str), reason: &str, u| {
        let i = id(at, n);
        seg(
            day,
            open(&i, at, agent, target) + &attempt(&i, by.0, by.1, by.2, reason) + &close(&i, by.0, by.1, by.2, u),
        );
    };
    finished(
        "2026-10-01",
        "2026-10-01T09:00:00Z",
        1,
        "ak_fixture1",
        "sonnet",
        ("anthropic", "main", "claude-sonnet-4-5"),
        "cold_by_deficit",
        usage.clone(),
    );
    finished(
        "2026-10-02",
        "2026-10-02T09:00:00Z",
        2,
        "ak_fixture2",
        "sonnet",
        ("anthropic", "main", "claude-sonnet-4-5"),
        "warm",
        serde_json::Value::Null,
    );
    finished(
        "2026-10-02",
        "2026-10-02T11:30:00Z",
        3,
        "ak_unnamed",
        "grok",
        ("xai", "work", "grok-4"),
        "payg_overflow",
        usage,
    );
    finished(
        "2026-10-03",
        "2026-10-03T09:00:00Z",
        4,
        "ak_fixture1",
        "sonnet",
        ("anthropic", "main", "claude-sonnet-4-5"),
        "warm",
        serde_json::Value::Null,
    );
}

/// The two unfinished records: 5 has an attempt behind its `open`, 6 only the `open`. They are
/// written apart from the rest because a server settles open records when it starts: the golden
/// suite appends them after starting one (they are then in flight) and before otherwise (cut short).
pub fn unfinished(h: &Path) {
    let seg = |day: &str, text: String| {
        let path = format!("records/{day}.jsonl");
        let old = fs::read_to_string(h.join(&path)).unwrap_or_default();
        put(h, &path, &(old + &text));
    };
    let cut = id("2026-10-03T13:00:00Z", 5);
    seg(
        "2026-10-03",
        open(&cut, "2026-10-03T13:00:00Z", "ak_fixture1", "sonnet")
            + &attempt(&cut, "anthropic", "main", "claude-sonnet-4-5", "warm"),
    );
    let live = id("2026-10-03T14:22:00Z", 6);
    seg("2026-10-03", open(&live, "2026-10-03T14:22:00Z", "ak_fixture1", "opus"));
}

/// Loads every file back through its loader: a fixture that doesn't parse is a bug here.
fn check_loads(h: &Path) {
    if h.join(accounts::FILE).exists() {
        Accounts::load(&h.join(accounts::FILE)).expect("fixture accounts load");
    }
    if h.join(tokens::FILE).exists() {
        TokenStore::load(h).expect("fixture tokens load");
    }
    if h.join(keys::FILE).exists() {
        Keys::load(&h.join(keys::FILE)).expect("fixture keys load");
    }
}
