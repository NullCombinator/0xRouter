//! The dashboard against the CLI (spec 014 T042, SC-002 and SC-003): every op id of
//! contracts/dashboard-forms.md is posted once to a running `serve` through `/do/...` on home A,
//! and its CLI twin runs once on home B, a copy of A taken before any change, with no server.
//!
//! After each step the two homes must hold the same files, with timestamps, random ids, secrets
//! and the random install id and routing salt left out, and dashboard-log.jsonl, the dashboard
//! token file, sockets, locks and pid files left out. Each refusal must read the same text: the
//! page's refusal notice on one side, the CLI's stderr on the other. A success is compared by its
//! files only, since the two report it in different words.
//!
//! Skipped, printed as `op: skipped (reason)` and listed here so the gap stays visible:
//! - accounts.signin, accounts.signin_paste: the OAuth sign-in reaches the identity provider.
//! - proxy.fixed, quota.poll: each starts a task that reaches the network.
//! - catalogue.install: downloads an adapter archive.
//! - adapters.review, adapters.build, adapters.rebuild: each starts a task that calls a model or
//!   the builder.
//! - test.run: makes billed model calls.

use std::collections::BTreeMap;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use nullrouter_registry::OperatorHome;
use nullrouter_server::views::{Live, keys as keys_view};
use reqwest::header::{CONTENT_TYPE, COOKIE, LOCATION, ORIGIN};
use reqwest::{Client, StatusCode, redirect};
use serde_json::{Value, json};

const ACCOUNT_BACK: &str = "/providers/anthropic";
const BACK: &str = "/endpoint";

/// One form post and its CLI twin. A `@key@` in the fields or the arguments is the id of `key`
/// in the home that receives it, since the two homes issued their keys separately.
struct Step {
    route: &'static str,
    fields: &'static [(&'static str, &'static str)],
    cli: &'static [&'static str],
    stdin: Option<&'static str>,
    key: Option<&'static str>,
}

struct Op {
    id: &'static str,
    back: &'static str,
    steps: &'static [Step],
}

const fn go(
    route: &'static str,
    fields: &'static [(&'static str, &'static str)],
    cli: &'static [&'static str],
) -> Step {
    Step { route, fields, cli, stdin: None, key: None }
}

const fn on_key(
    key: &'static str,
    route: &'static str,
    fields: &'static [(&'static str, &'static str)],
    cli: &'static [&'static str],
) -> Step {
    Step { key: Some(key), ..go(route, fields, cli) }
}

/// Every op id of contracts/dashboard-forms.md that the suite runs, in the order it runs them.
/// Later steps need what earlier ones made (accounts before routing, keys before their tags).
const OPS: &[Op] = &[
    Op {
        id: "accounts.add",
        back: ACCOUNT_BACK,
        steps: &[
            Step {
                stdin: Some("sk-work-0000001\n"),
                ..go(
                    "accounts/add",
                    &[("provider", "anthropic"), ("name", "work"), ("secret", "sk-work-0000001"), ("order", "")],
                    &["accounts", "add", "anthropic", "work"],
                )
            },
            Step {
                stdin: Some("sk-side-0000002\n"),
                ..go(
                    "accounts/add",
                    &[("provider", "anthropic"), ("name", "side"), ("secret", "sk-side-0000002"), ("order", "")],
                    &["accounts", "add", "anthropic", "side"],
                )
            },
            Step {
                stdin: Some("sk-bad-0000004\n"),
                ..go(
                    "accounts/add",
                    &[("provider", "anthropic"), ("name", "Bad Name"), ("secret", "sk-bad-0000004"), ("order", "")],
                    &["accounts", "add", "anthropic", "Bad Name"],
                )
            },
        ],
    },
    Op {
        id: "accounts.priority",
        back: ACCOUNT_BACK,
        steps: &[
            go(
                "accounts/priority",
                &[("provider", "anthropic"), ("name", "work"), ("priority", "2")],
                &["accounts", "priority", "anthropic", "work", "2"],
            ),
            go(
                "accounts/priority",
                &[("provider", "anthropic"), ("name", "work"), ("priority", "not-a-number")],
                &["accounts", "priority", "anthropic", "work", "not-a-number"],
            ),
        ],
    },
    Op {
        id: "accounts.disable",
        back: ACCOUNT_BACK,
        steps: &[go(
            "accounts/disable",
            &[("provider", "anthropic"), ("name", "side")],
            &["accounts", "disable", "anthropic", "side"],
        )],
    },
    Op {
        id: "accounts.enable",
        back: ACCOUNT_BACK,
        steps: &[go(
            "accounts/enable",
            &[("provider", "anthropic"), ("name", "side")],
            &["accounts", "enable", "anthropic", "side"],
        )],
    },
    Op {
        id: "accounts.exclusive",
        back: ACCOUNT_BACK,
        steps: &[
            go(
                "accounts/exclusive",
                &[("provider", "anthropic"), ("name", "work"), ("on", "on")],
                &["accounts", "exclusive", "anthropic", "work", "on"],
            ),
            go(
                "accounts/exclusive",
                &[("provider", "anthropic"), ("name", "work")],
                &["accounts", "exclusive", "anthropic", "work", "off"],
            ),
        ],
    },
    Op {
        id: "accounts.remove",
        back: ACCOUNT_BACK,
        steps: &[go(
            "accounts/remove",
            &[("provider", "anthropic"), ("name", "side")],
            &["accounts", "remove", "anthropic", "side"],
        )],
    },
    Op {
        id: "keys.issue",
        back: BACK,
        steps: &[
            go(
                "keys/issue",
                &[("name", "laptop"), ("break", "restart"), ("harness", "claude-code"), ("adapter", "")],
                &["keys", "issue", "laptop", "--break", "restart", "--harness", "claude-code"],
            ),
            go(
                "keys/issue",
                &[("name", "ci"), ("break", ""), ("harness", ""), ("adapter", "")],
                &["keys", "issue", "ci"],
            ),
        ],
    },
    Op {
        id: "keys.tag",
        back: BACK,
        steps: &[
            on_key(
                "ci",
                "keys/tag",
                &[("key", "@key@"), ("text", "nightly")],
                &["keys", "tag", "@key@", "nightly"],
            ),
            on_key("ci", "keys/tag", &[("key", "@key@"), ("text", "")], &["keys", "tag", "@key@", "--clear"]),
        ],
    },
    Op {
        id: "keys.set_break",
        back: BACK,
        steps: &[on_key(
            "ci",
            "keys/set-break",
            &[("key", "@key@"), ("behaviour", "error_event")],
            &["keys", "set-break", "@key@", "error_event"],
        )],
    },
    Op {
        id: "keys.set_adapter",
        back: BACK,
        steps: &[
            on_key(
                "laptop",
                "keys/set-adapter",
                &[("key", "@key@"), ("harness", "claude-code")],
                &["keys", "set-adapter", "@key@", "claude-code"],
            ),
            on_key(
                "laptop",
                "keys/set-adapter",
                &[("key", "@key@"), ("harness", "")],
                &["keys", "set-adapter", "@key@", "--clear"],
            ),
        ],
    },
    Op {
        id: "keys.revoke",
        back: BACK,
        steps: &[on_key("ci", "keys/revoke", &[("key", "@key@")], &["keys", "revoke", "@key@"])],
    },
    Op {
        id: "behaviour.set_break",
        back: BACK,
        steps: &[go("behaviour/set-break", &[("behaviour", "error_event")], &["behaviour", "set-break", "error_event"])],
    },
    Op {
        id: "routing.set",
        back: ACCOUNT_BACK,
        steps: &[
            go(
                "routing/set",
                &[("provider", "anthropic"), ("name", "work"), ("key", "cache_lifetime"), ("value", "1h")],
                &["routing", "set", "anthropic", "work", "cache_lifetime=1h"],
            ),
            go(
                "routing/set",
                &[("provider", "anthropic"), ("name", "work"), ("key", "reserve"), ("value", "ten")],
                &["routing", "set", "anthropic", "work", "reserve=ten"],
            ),
        ],
    },
    Op {
        id: "routing.unset",
        back: ACCOUNT_BACK,
        steps: &[go(
            "routing/unset",
            &[("provider", "anthropic"), ("name", "work"), ("key", "cache_lifetime")],
            &["routing", "unset", "anthropic", "work", "cache_lifetime"],
        )],
    },
    Op {
        id: "routing.set_plugin",
        back: ACCOUNT_BACK,
        steps: &[go(
            "routing/set-plugin",
            &[("provider", "anthropic"), ("key", "reserve"), ("value", "5%")],
            &["routing", "set-plugin", "anthropic", "reserve=5%"],
        )],
    },
    Op {
        id: "routing.unset_plugin",
        back: ACCOUNT_BACK,
        steps: &[go(
            "routing/unset-plugin",
            &[("provider", "anthropic"), ("key", "reserve")],
            &["routing", "unset-plugin", "anthropic", "reserve"],
        )],
    },
    Op {
        id: "routing.window",
        back: BACK,
        steps: &[go("routing/window", &[("target", ""), ("duration", "2h")], &["routing", "window", "2h"])],
    },
    Op {
        id: "connection.set",
        back: ACCOUNT_BACK,
        steps: &[
            go(
                "connection/set",
                &[("provider", "anthropic"), ("model", ""), ("key", "headers"), ("value", "30s")],
                &["connection", "set", "anthropic", "headers", "30s"],
            ),
            go(
                "connection/set",
                &[("provider", "anthropic"), ("model", ""), ("key", "headers"), ("value", "soon")],
                &["connection", "set", "anthropic", "headers", "soon"],
            ),
        ],
    },
    Op {
        id: "connection.unset",
        back: ACCOUNT_BACK,
        steps: &[go(
            "connection/unset",
            &[("provider", "anthropic"), ("model", ""), ("key", "headers")],
            &["connection", "unset", "anthropic", "headers"],
        )],
    },
    Op {
        id: "proxy.add",
        back: BACK,
        steps: &[go(
            "proxy/add",
            &[("name", "eu"), ("url", "http://proxy.example:3128"), ("username", ""), ("password", "")],
            &["proxy", "add", "eu", "http://proxy.example:3128"],
        )],
    },
    Op {
        id: "proxy.use",
        back: BACK,
        steps: &[
            go(
                "proxy/use",
                &[("proxy", "eu"), ("scope", "provider"), ("target", "anthropic")],
                &["proxy", "use", "eu", "--provider", "anthropic"],
            ),
            go(
                "proxy/use",
                &[("proxy", "eu"), ("scope", "account"), ("target", "anthropic/work")],
                &["proxy", "use", "eu", "--account", "anthropic/work"],
            ),
        ],
    },
    Op {
        id: "proxy.clear",
        back: BACK,
        steps: &[go(
            "proxy/clear",
            &[("scope", "provider"), ("target", "anthropic")],
            &["proxy", "clear", "--provider", "anthropic"],
        )],
    },
    Op {
        id: "proxy.remove",
        back: BACK,
        steps: &[go("proxy/remove", &[("name", "eu")], &["proxy", "remove", "eu"])],
    },
    Op {
        id: "quota.interval",
        back: BACK,
        steps: &[go(
            "quota/interval",
            &[("provider", "anthropic"), ("name", "work"), ("every", "15m")],
            &["quota", "interval", "anthropic", "work", "15m"],
        )],
    },
    Op {
        id: "quota.ack",
        back: BACK,
        steps: &[go("quota/ack", &[("id", "all")], &["quota", "ack", "all"])],
    },
    Op {
        id: "plugins.install",
        back: BACK,
        steps: &[go("plugins/install", &[("id", "baidu")], &["plugins", "install", "baidu"])],
    },
    Op {
        id: "providers.disable",
        back: BACK,
        steps: &[go("providers/disable", &[("id", "baidu")], &["providers", "disable", "baidu"])],
    },
    Op {
        id: "providers.enable",
        back: BACK,
        steps: &[go("providers/enable", &[("id", "baidu")], &["providers", "enable", "baidu"])],
    },
    Op {
        id: "providers.hide",
        back: BACK,
        steps: &[go("providers/hide", &[("id", "baidu")], &["providers", "hide", "baidu"])],
    },
    Op {
        id: "providers.show",
        back: BACK,
        steps: &[go("providers/show", &[("id", "baidu")], &["providers", "show", "baidu"])],
    },
    Op {
        id: "providers.add_compatible",
        back: BACK,
        steps: &[
            // Private endpoints are still off here, so the loopback address is refused.
            go(
                "providers/add-compatible",
                &[
                    ("id", "local-llm"),
                    ("name", ""),
                    ("style", "openai"),
                    ("base_url", "http://127.0.0.1:11434"),
                    ("models", "llama3"),
                ],
                &[
                    "providers",
                    "add-compatible",
                    "local-llm",
                    "--style",
                    "openai",
                    "--base-url",
                    "http://127.0.0.1:11434",
                    "--model",
                    "llama3",
                ],
            ),
            go(
                "providers/add-compatible",
                &[
                    ("id", "local-llm"),
                    ("name", ""),
                    ("style", "openai"),
                    ("base_url", "https://llm.example.com/v1"),
                    ("models", "a\nb\nc"),
                ],
                &[
                    "providers",
                    "add-compatible",
                    "local-llm",
                    "--style",
                    "openai",
                    "--base-url",
                    "https://llm.example.com/v1",
                    "--model",
                    "a",
                    "--model",
                    "b",
                    "--model",
                    "c",
                ],
            ),
        ],
    },
    Op {
        id: "plugins.uninstall",
        back: BACK,
        steps: &[go("plugins/uninstall", &[("id", "baidu")], &["plugins", "uninstall", "baidu"])],
    },
    Op {
        id: "plugins.private_endpoints",
        back: BACK,
        steps: &[
            go("plugins/private-endpoints", &[("on", "on")], &["plugins", "private-endpoints", "on", "--yes"]),
            go("plugins/private-endpoints", &[], &["plugins", "private-endpoints", "off"]),
        ],
    },
    Op {
        id: "adapters.install",
        back: BACK,
        steps: &[go(
            "adapters/install",
            &[("path", "/nonexistent/claude-code-0.1.0.tar")],
            &["adapters", "install", "/nonexistent/claude-code-0.1.0.tar"],
        )],
    },
    Op {
        id: "adapters.review_settings",
        back: BACK,
        steps: &[
            go("adapters/review-settings", &[("clear", "on")], &["adapters", "review-settings", "--clear"]),
            go(
                "adapters/review-settings",
                &[("model", "m1"), ("budget", "1000"), ("reserve_output", ""), ("clear", "")],
                &["adapters", "review-settings", "--model", "m1", "--budget", "1000"],
            ),
        ],
    },
    Op {
        id: "adapters.approve",
        back: BACK,
        steps: &[go(
            "adapters/approve",
            &[("harness", "claude-code"), ("version", "0.1.0"), ("note", "")],
            &["adapters", "approve", "claude-code", "0.1.0"],
        )],
    },
    Op {
        id: "adapters.reject",
        back: BACK,
        steps: &[go(
            "adapters/reject",
            &[("harness", "claude-code"), ("version", "0.1.0"), ("note", "")],
            &["adapters", "reject", "claude-code", "0.1.0"],
        )],
    },
    Op {
        id: "adapters.clear",
        back: BACK,
        steps: &[go(
            "adapters/clear",
            &[("harness", "claude-code"), ("version", "0.1.0")],
            &["adapters", "clear", "claude-code", "0.1.0", "--yes"],
        )],
    },
    Op {
        id: "adapters.remove",
        back: BACK,
        steps: &[go(
            "adapters/remove",
            &[("harness", "claude-code"), ("version", "0.1.0")],
            &["adapters", "remove", "claude-code", "0.1.0", "--yes"],
        )],
    },
    Op {
        id: "alerts.ack",
        back: BACK,
        steps: &[go("alerts/ack", &[("id", "all")], &["alerts", "ack", "--all"])],
    },
    Op {
        id: "unified.add",
        back: BACK,
        steps: &[
            go(
                "unified/add",
                &[("name", "u"), ("members", "openrouter/x/a\nopenrouter/x/b")],
                &["unified", "add", "u", "openrouter/x/a", "openrouter/x/b"],
            ),
            go("unified/add", &[("name", "u"), ("members", "openrouter/x/a")], &["unified", "add", "u", "openrouter/x/a"]),
        ],
    },
    Op {
        id: "unified.set",
        back: BACK,
        steps: &[go("unified/set", &[("name", "u"), ("members", "openrouter/x/c")], &["unified", "set", "u", "openrouter/x/c"])],
    },
    Op {
        id: "combos.add",
        back: BACK,
        steps: &[
            go("combos/add", &[("name", "fast"), ("members", "x")], &["combos", "add", "fast", "x"]),
            go("combos/add", &[("name", "fast"), ("members", "u")], &["combos", "add", "fast", "u"]),
        ],
    },
    Op {
        id: "combos.set",
        back: BACK,
        steps: &[go("combos/set", &[("name", "fast"), ("members", "u")], &["combos", "set", "fast", "u"])],
    },
    Op {
        id: "combos.remove",
        back: BACK,
        steps: &[go("combos/remove", &[("name", "fast")], &["combos", "remove", "fast", "--yes"])],
    },
    // Last among the unified models, since the combo above used `u`.
    Op {
        id: "unified.remove",
        back: BACK,
        steps: &[go("unified/remove", &[("name", "u")], &["unified", "remove", "u", "--yes"])],
    },
    Op {
        id: "verdicts.mark",
        back: ACCOUNT_BACK,
        steps: &[go(
            "verdicts/mark",
            &[("provider", "anthropic"), ("account", "work"), ("model", "m1"), ("note", "checked by hand")],
            &["verdicts", "mark", "anthropic", "work", "m1", "--note", "checked by hand"],
        )],
    },
    Op {
        id: "verdicts.clear",
        back: ACCOUNT_BACK,
        steps: &[go(
            "verdicts/clear",
            &[("provider", "anthropic"), ("account", "work"), ("model", "m1")],
            &["verdicts", "clear", "anthropic", "work", "m1"],
        )],
    },
    Op {
        id: "verdicts.settings",
        back: BACK,
        steps: &[
            go("verdicts/settings", &[("setting", "concurrency"), ("value", "2")], &["verdicts", "settings", "concurrency", "2"]),
            go(
                "verdicts/settings",
                &[("setting", "timeout"), ("type", "image"), ("value", "10m")],
                &["verdicts", "settings", "timeout", "image", "10m"],
            ),
        ],
    },
];

/// The op ids skipped, and why.
const SKIPS: &[(&str, &str)] = &[
    ("accounts.signin", "OAuth sign-in reaches the identity provider"),
    ("accounts.signin_paste", "OAuth sign-in pastes a code from the identity provider"),
    ("proxy.fixed", "starts a task that reaches the network"),
    ("quota.poll", "starts a task that polls the provider's quota API over the network"),
    ("catalogue.install", "downloads an adapter archive over the network"),
    ("adapters.review", "starts a task that makes a model call"),
    ("adapters.build", "starts a task that runs the builder"),
    ("adapters.rebuild", "starts a task that runs the builder"),
    ("test.run", "makes billed model calls"),
];

/// Every op id of contracts/dashboard-forms.md; each must be run or skipped.
const CONTRACT_OPS: &[&str] = &[
    "accounts.add",
    "accounts.signin",
    "accounts.signin_paste",
    "accounts.priority",
    "accounts.enable",
    "accounts.disable",
    "accounts.exclusive",
    "accounts.remove",
    "keys.issue",
    "keys.revoke",
    "keys.tag",
    "keys.set_break",
    "keys.set_adapter",
    "behaviour.set_break",
    "routing.set",
    "routing.unset",
    "routing.set_plugin",
    "routing.unset_plugin",
    "routing.window",
    "connection.set",
    "connection.unset",
    "proxy.add",
    "proxy.remove",
    "proxy.use",
    "proxy.clear",
    "proxy.fixed",
    "quota.interval",
    "quota.poll",
    "quota.ack",
    "plugins.install",
    "plugins.uninstall",
    "plugins.private_endpoints",
    "providers.enable",
    "providers.disable",
    "providers.hide",
    "providers.show",
    "providers.add_compatible",
    "adapters.install",
    "catalogue.install",
    "adapters.review",
    "adapters.build",
    "adapters.rebuild",
    "adapters.approve",
    "adapters.reject",
    "adapters.clear",
    "adapters.remove",
    "adapters.review_settings",
    "alerts.ack",
    "unified.add",
    "unified.set",
    "unified.remove",
    "combos.add",
    "combos.set",
    "combos.remove",
    "test.run",
    "verdicts.mark",
    "verdicts.clear",
    "verdicts.settings",
];

/// What a post or a CLI run came to: done, or refused with this text.
#[derive(Debug, PartialEq)]
enum Outcome {
    Done,
    Refused(String),
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn cli<S: AsRef<str>>(home: &Path, args: &[S], stdin: Option<&str>) -> Out {
    let argv: Vec<&str> = args.iter().map(|s| <S as AsRef<str>>::as_ref(s)).collect();
    let mut child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    if let Some(s) = stdin {
        // A command that exits without reading its stdin closes the pipe first; that is no failure.
        let _ = pipe.write_all(s.as_bytes());
    }
    drop(pipe);
    let o = child.wait_with_output().unwrap();
    Out {
        ok: o.status.success(),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

/// The id of the key named `name` in `home`.
fn key_id(home: &Path, name: &str) -> String {
    let keys = keys_view::build(&OperatorHome::new(home), &json!({}), &Live::none()).expect("the keys view").json;
    keys.as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == name)
        .and_then(|k| k["id"].as_str())
        .unwrap_or_else(|| panic!("no key named {name}"))
        .to_owned()
}

fn fill(v: &str, key: Option<&str>) -> String {
    if v == "@key@" { key.expect("a step with @key@ names its key").to_owned() } else { v.to_owned() }
}

/// Collapses whitespace, so a refusal reads the same whatever the line breaks.
fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` with `home`'s path written `<home>`, so twin homes' refusals compare.
fn homeless(text: &str, home: &Path) -> String {
    text.replace(&home.display().to_string(), "<home>")
}

/// A copy of `from`: the regular files and the directories, not sockets.
fn copy_tree(from: &Path, to: &Path) {
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let kind = e.file_type().unwrap();
        if kind.is_dir() {
            std::fs::create_dir_all(&dst).unwrap();
            copy_tree(&src, &dst);
        } else if kind.is_file() {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

/// Whether a file takes part in the comparison (`rel` is relative to the home). The running
/// server's own state (the routing ledger and warm list, the quota fits) is not an op's change.
fn compared(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    !(name == "dashboard-log.jsonl"
        || name == "dashboard.toml"
        || name == "install-id"
        || rel.starts_with("routing/")
        || rel.starts_with("quota/fit/")
        || [".sock", ".pid", ".lock"].iter().any(|s| name.ends_with(s)))
}

/// Keys whose values are random per home (ids, digests) or secret: masked whatever they hold.
const MASKED: &[&str] = &["id", "digest", "secret", "last4"];

/// A value with its timestamps shown as `<ts>` and its random keys as `<masked>`.
fn normal(v: &Value, key: &str) -> Value {
    match v {
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), normal(v, k))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(|v| normal(v, key)).collect()),
        Value::String(_) if MASKED.contains(&key) => json!("<masked>"),
        Value::String(s) if s.parse::<jiff::Timestamp>().is_ok() => json!("<ts>"),
        other => other.clone(),
    }
}

fn stamp_at(b: &[u8], i: usize) -> bool {
    b.len() >= i + 11
        && b[i..i + 4].iter().all(u8::is_ascii_digit)
        && b[i + 4] == b'-'
        && b[i + 7] == b'-'
        && b[i + 10] == b'T'
}

/// Text with every RFC 3339 timestamp shown as `<ts>`.
fn stamps(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if stamp_at(b, i) {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_digit() || b":.-+TZ".contains(&b[j])) {
                j += 1;
            }
            out.push_str("<ts>");
            i = j;
        } else {
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// A file's comparable form: TOML as normalised data, anything else with its timestamps masked.
fn normal_text(rel: &str, bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).into_owned();
    if rel.ends_with(".toml") {
        if let Ok(table) = toml::from_str::<toml::Table>(&text) {
            let v = serde_json::to_value(table).unwrap();
            return serde_json::to_string_pretty(&normal(&v, "")).unwrap();
        }
    }
    stamps(&text)
}

/// Every compared file of `home`, normalised, by its path relative to the home.
fn snapshot(home: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![home.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let e = e.unwrap();
            let path = e.path();
            let kind = e.file_type().unwrap();
            if kind.is_dir() {
                // The running server's sockets and pid files live under run/.
                if e.file_name().to_str() != Some("run") {
                    stack.push(path);
                }
            } else if kind.is_file() {
                let rel = path.strip_prefix(home).unwrap().display().to_string();
                if compared(&rel) {
                    let bytes = std::fs::read(&path).unwrap();
                    out.insert(rel.clone(), normal_text(&rel, &bytes));
                }
            }
        }
    }
    out
}

fn free_ports() -> (u16, u16) {
    let a = TcpListener::bind("127.0.0.1:0").unwrap();
    let b = TcpListener::bind("127.0.0.1:0").unwrap();
    (a.local_addr().unwrap().port(), b.local_addr().unwrap().port())
}

fn config(dash: u16) -> String {
    format!("schema = 1\n\n[dashboard]\nlisten = \"127.0.0.1:{dash}\"\n")
}

struct Serving(Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A running `serve` on `home`: the client listener on `client`, the dashboard on `dash`.
fn serve(home: &Path, client: u16, dash: u16) -> Serving {
    let child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(["serve", "--listen", &format!("127.0.0.1:{client}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let serving = Serving(child);
    for _ in 0..500 {
        if home.join("run/operator.sock").exists()
            && TcpStream::connect(("127.0.0.1", client)).is_ok()
            && TcpStream::connect(("127.0.0.1", dash)).is_ok()
        {
            return serving;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the server did not start");
}

/// The text of the first flash notice on a page: whether it is a refusal, and its text.
fn notice(page: &str) -> Option<(bool, String)> {
    let marker = "<p class=\"notice__text\">";
    let at = page.find(marker)?;
    let rest = &page[at + marker.len()..];
    let text = decode(&rest[..rest.find("</p>")?]);
    let start = page[..at].rfind("class=\"notice ")?;
    Some((page[start..at].contains("notice--error"), text))
}

/// Decodes the entities maud writes.
fn decode(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

/// Percent-encodes a form value.
fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A signed-in browser on the dashboard of a running server.
struct Site {
    http: Client,
    base: String,
    origin: String,
    cookie: String,
    token: String,
}

impl Site {
    /// The session is the token the CLI issued; the form token is read from the endpoint page,
    /// where the keys forms carry it.
    async fn start(dash: u16, session: &str) -> Self {
        let http = Client::builder()
            .redirect(redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();
        let mut site = Self {
            http,
            base: format!("http://127.0.0.1:{dash}"),
            origin: format!("http://127.0.0.1:{dash}"),
            cookie: format!("nrd_session_{dash}={session}"),
            token: String::new(),
        };
        let (status, body) = site.get(BACK).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let marker = "name=\"_t\" value=\"";
        let at = body.find(marker).expect("the endpoint page carries the form token");
        let rest = &body[at + marker.len()..];
        site.token = rest[..rest.find('"').unwrap()].to_owned();
        site
    }

    async fn get(&self, path: &str) -> (StatusCode, String) {
        let r = self.http.get(format!("{}{path}", self.base)).header(COOKIE, &self.cookie).send().await.unwrap();
        let status = r.status();
        (status, r.text().await.unwrap())
    }

    /// Posts `fields` to `/do/<route>` as the page's form does, follows the 303 and reads the
    /// flash: a refusal's text, or done.
    async fn post(&self, route: &str, back: &str, fields: &[(&str, String)]) -> Outcome {
        let mut body = format!("_t={}&_back={}", enc(&self.token), enc(back));
        for (k, v) in fields {
            body.push_str(&format!("&{k}={}", enc(v)));
        }
        let r = self
            .http
            .post(format!("{}/do/{route}", self.base))
            .header(COOKIE, &self.cookie)
            .header(ORIGIN, &self.origin)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER, "{route}");
        let location = r.headers()[LOCATION].to_str().unwrap().to_owned();
        assert!(
            location.starts_with(&format!("{back}?done=")) || location.starts_with(&format!("{back}?window=")),
            "{route} went to {location}"
        );
        let (status, page) = self.get(&location).await;
        assert_eq!(status, StatusCode::OK, "{location}");
        match notice(&page) {
            Some((true, text)) => Outcome::Refused(words(&text)),
            Some((false, _)) => Outcome::Done,
            None => {
                assert!(page.contains("once__value"), "{route}: no flash on {location}");
                Outcome::Done
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_op_through_the_dashboard_leaves_the_same_homes_as_its_cli_twin() {
    for id in CONTRACT_OPS {
        assert!(OPS.iter().any(|o| o.id == *id) || SKIPS.iter().any(|s| s.0 == *id), "{id} has no entry");
    }

    let a = tempfile::tempdir().unwrap();
    let (client, dash) = free_ports();
    std::fs::write(a.path().join("config.toml"), config(dash)).unwrap();
    let issued = cli(a.path(), &["dashboard", "token"], None);
    assert!(issued.ok, "{}", issued.stderr);
    let session = issued.stdout.trim_end().to_owned();

    // The twin is taken before any change, and before the server starts.
    let b = tempfile::tempdir().unwrap();
    copy_tree(a.path(), b.path());
    let _serving = serve(a.path(), client, dash);
    let site = Site::start(dash, &session).await;

    for op in OPS {
        for (n, step) in op.steps.iter().enumerate() {
            let n = n + 1;
            let key_a = step.key.map(|k| key_id(a.path(), k));
            let key_b = step.key.map(|k| key_id(b.path(), k));

            let fields: Vec<(&str, String)> =
                step.fields.iter().map(|(k, v)| (*k, fill(v, key_a.as_deref()))).collect();
            let browser = site.post(step.route, op.back, &fields).await;

            let args: Vec<String> = step.cli.iter().map(|v| fill(v, key_b.as_deref())).collect();
            let twin = cli(b.path(), args.as_slice(), step.stdin);
            let twin = if twin.ok { Outcome::Done } else { Outcome::Refused(homeless(&words(&twin.stderr), b.path())) };
            let browser = match browser {
                Outcome::Refused(text) => Outcome::Refused(homeless(&text, a.path())),
                done => done,
            };

            assert_eq!(browser, twin, "{} step {n}: the dashboard and the CLI disagree", op.id);
            assert_eq!(snapshot(a.path()), snapshot(b.path()), "{} step {n}: the homes differ", op.id);
        }
        println!("{}: browser == cli", op.id);
    }

    for (id, reason) in SKIPS {
        println!("{id}: skipped ({reason})");
    }
    // The comparison must not pass on two empty homes.
    assert!(a.path().join("accounts.toml").exists() && a.path().join("keys.toml").exists());
}
