//! US3 (spec 014, T031): the routing-target forms and the model test through the browser.
//!
//! The target forms run against the CLI's ops (`nullrouter_server::ops::targets`) on a twin home,
//! as the US1 and US2 suites do: each refusal reads the CLI's text, each success its line (the
//! only word that may differ is "applied" for "saved; applies at next start"), and the two homes
//! hold the same `config.toml` after the steps.
//!
//! The model test runs against a mock upstream. The bundled `anthropic` provider's endpoint is the
//! real API, so the home replaces it (`plugin_decisions`, `replace`) with a plugin of the same id
//! pointed at the mock; the members stay `anthropic/m1`. A server-less CLI cannot plan a test, so
//! the two-call question is the plan the running server makes for the same target
//! (`model_tests::plan`), whose `question()` is the text the CLI prints before ` [y/N] `.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{Site, ctx_of, notice, text_of, twin_of};
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_server::ops::targets::{self, Kind};
use nullrouter_server::ops::{Ctx, Done, OpError, model_tests};
use nullrouter_server::views::{fetch_in_process, verdicts};
use reqwest::StatusCode;
use reqwest::header::LOCATION;
use serde_json::json;

const SECRET_MAIN: &str = "sk-fixture-main-AAAA0001";
const SECRET_BACKUP: &str = "sk-fixture-backup-BBBB0002";
const RUN: &str = "/do/test/run";

/// The CLI's op on a context, returning its result.
type Cli = fn(&Ctx) -> Result<Done, OpError>;

/// One target operation: the form's route, the page it returns to, the fields the form posts, and
/// the CLI's op with the same arguments.
struct Op {
    route: &'static str,
    back: &'static str,
    fields: &'static [(&'static str, &'static str)],
    cli: Cli,
}

/// `unified add|set` and `combos add|set` as the CLI calls them.
fn add_op(c: &Ctx, kind: Kind, name: &str, members: &[&str]) -> Result<Done, OpError> {
    let list: Vec<String> = members.iter().map(|m| (*m).to_owned()).collect();
    targets::add_with(c, &mut Vec::new(), kind, name, &list).map(|r| r.done)
}

fn set_op(c: &Ctx, kind: Kind, name: &str, members: &[&str]) -> Result<Done, OpError> {
    let list: Vec<String> = members.iter().map(|m| (*m).to_owned()).collect();
    targets::set_with(c, &mut Vec::new(), kind, name, &list).map(|r| r.done)
}

fn remove_op(c: &Ctx, kind: Kind, name: &str) -> Result<Done, OpError> {
    targets::remove_with(c, &mut Vec::new(), kind, name).map(|r| r.done)
}

/// The steps of US3's declare, change and remove forms, in the order an operator would take them.
/// Refusals sit where the state they are refused in is set up.
fn target_ops() -> Vec<Op> {
    vec![
        Op {
            route: "unified/add",
            back: "/combo",
            fields: &[("name", "fast"), ("members", "anthropic/m1")],
            cli: |c| add_op(c, Kind::Unified, "fast", &["anthropic/m1"]),
        },
        // Declared already: the same add is refused.
        Op {
            route: "unified/add",
            back: "/combo",
            fields: &[("name", "fast"), ("members", "anthropic/m1")],
            cli: |c| add_op(c, Kind::Unified, "fast", &["anthropic/m1"]),
        },
        Op {
            route: "unified/set",
            back: "/combo",
            fields: &[("name", "fast"), ("members", "nope/m")],
            cli: |c| set_op(c, Kind::Unified, "fast", &["nope/m"]),
        },
        Op {
            route: "unified/set",
            back: "/combo",
            fields: &[("name", "fast"), ("members", "anthropic/m1")],
            cli: |c| set_op(c, Kind::Unified, "fast", &["anthropic/m1"]),
        },
        Op {
            route: "combos/add",
            back: "/combo",
            fields: &[("name", "c1"), ("members", "fast")],
            cli: |c| add_op(c, Kind::Combo, "c1", &["fast"]),
        },
        Op {
            route: "combos/add",
            back: "/combo",
            fields: &[("name", "c2"), ("members", "ghost")],
            cli: |c| add_op(c, Kind::Combo, "c2", &["ghost"]),
        },
        Op {
            route: "combos/set",
            back: "/combo",
            fields: &[("name", "c1"), ("members", "fast\nu")],
            cli: |c| set_op(c, Kind::Combo, "c1", &["fast", "u"]),
        },
        // Combo c1 lists it: refused until the combo goes.
        Op {
            route: "unified/remove",
            back: "/combo",
            fields: &[("name", "fast")],
            cli: |c| remove_op(c, Kind::Unified, "fast"),
        },
        Op {
            route: "combos/remove",
            back: "/combo",
            fields: &[("name", "c1")],
            cli: |c| remove_op(c, Kind::Combo, "c1"),
        },
        Op {
            route: "unified/remove",
            back: "/combo",
            fields: &[("name", "fast")],
            cli: |c| remove_op(c, Kind::Unified, "fast"),
        },
    ]
}

/// A mock's answer to a text request: a chat completion with one reply.
fn reply() -> Step {
    Step::json(
        200,
        json!({"id": "up-1", "object": "chat.completion", "created": 1, "model": "m1",
               "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}],
               "usage": {"prompt_tokens": 3, "completion_tokens": 1}}),
    )
}

async fn start_mock() -> MockUpstream {
    let mock = MockUpstream::start().await;
    mock.respond(|_| reply());
    mock
}

/// A home whose `anthropic` answers at the mock (replacing the bundled one), two accounts so that a
/// test of `u` bills two calls, and the unified model `u` with the one member `anthropic/m1`.
fn home_at(mock: &MockUpstream) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    std::fs::create_dir(h.join("plugins")).unwrap();
    std::fs::write(
        h.join("plugins/anthropic.toml"),
        format!(
            "schema = 2\nid = \"anthropic\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n",
            mock.url("/anthropic/chat/completions")
        ),
    )
    .unwrap();
    std::fs::write(
        h.join("config.toml"),
        "allow_private_endpoints = true\n\n[plugin_decisions]\nanthropic = \"replace\"\n\n\
         [[unified_model]]\nname = \"u\"\nmembers = [{ provider = \"anthropic\", model = \"m1\" }]\n",
    )
    .unwrap();
    let file = h.join(nullrouter_engine::accounts::FILE);
    std::fs::write(
        &file,
        format!(
            "schema = 2\n\n[[account]]\nprovider = \"anthropic\"\nname = \"main\"\nsecret = \"{SECRET_MAIN}\"\norder = 1\n\n\
             [[account]]\nprovider = \"anthropic\"\nname = \"backup\"\nsecret = \"{SECRET_BACKUP}\"\norder = 2\n"
        ),
    )
    .unwrap();
    // The engine refuses a secret file that other users can read.
    std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    dir
}

/// The CLI's line with the one word a server-less CLI differs on.
fn cli_line(d: &Done) -> String {
    d.text.replace("saved; applies at next start", "applied")
}

fn config(home: &Path) -> String {
    std::fs::read_to_string(home.join("config.toml")).unwrap_or_default()
}

/// The first form posting to `action`, through its `</form>`.
fn form_of<'a>(html: &'a str, action: &str) -> &'a str {
    let at = html.find(&format!("action=\"{action}\"")).unwrap_or_else(|| panic!("no form posts to {action}"));
    let start = html[..at].rfind("<form").expect("the action belongs to a form");
    let end = at + html[at..].find("</form>").expect("the form closes");
    &html[start..end]
}

/// Posts `op` through the browser and runs the CLI's op on the twin: the page's flash must read what
/// the CLI printed (its refusal, or its line). Returns the refusal's text, if it was refused.
async fn through(site: &Site, twin: &Path, op: &Op) -> Option<String> {
    let fields: Vec<(&str, String)> = op.fields.iter().map(|(k, v)| (*k, (*v).to_owned())).collect();
    let flash = site.flow(op.route, op.back, &fields).await;
    let (is_error, shown) = notice(&flash.page).expect("the flash notice");
    match (op.cli)(&ctx_of(twin)) {
        Err(e) => {
            assert!(is_error, "{}: the CLI refused, the page did not: {shown}", op.route);
            assert_eq!(shown, e.text, "{} {:?}: the page and the CLI disagree", op.route, op.fields);
            Some(e.text)
        }
        Ok(d) => {
            assert!(!is_error, "{}: the page refused, the CLI did not: {shown}", op.route);
            assert_eq!(shown, cli_line(&d), "{} {:?}: the page and the CLI disagree", op.route, op.fields);
            None
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_target_op_through_the_browser_leaves_the_same_state_as_the_cli() {
    let mock = start_mock().await;
    let site = Site::start_on(home_at(&mock)).await;
    let twin = twin_of(site.home());
    let ops = target_ops();
    let mut refusals = 0;
    for op in &ops {
        if through(&site, twin.path(), op).await.is_some() {
            refusals += 1;
        }
    }
    // The comparison must cover refusals and successes alike.
    assert!(refusals > 0 && refusals < ops.len(), "{refusals} refusals");
    assert_eq!(config(site.home()), config(twin.path()), "config.toml differs");
    assert!(mock.received().is_empty(), "a target change called the upstream");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_member_shows_the_refusal() {
    let mock = start_mock().await;
    let site = Site::start_on(home_at(&mock)).await;
    let twin = twin_of(site.home());
    let before = config(site.home());

    let combo = Op {
        route: "combos/add",
        back: "/combo",
        fields: &[("name", "c2"), ("members", "ghost")],
        cli: |c| add_op(c, Kind::Combo, "c2", &["ghost"]),
    };
    let shown = through(&site, twin.path(), &combo).await.expect("a combo's unknown member is refused");
    assert!(shown.contains("ghost"), "{shown}");

    let unified = Op {
        route: "unified/add",
        back: "/combo",
        fields: &[("name", "x"), ("members", "nope/m")],
        cli: |c| add_op(c, Kind::Unified, "x", &["nope/m"]),
    };
    let shown = through(&site, twin.path(), &unified).await.expect("an unknown provider is refused");
    assert!(shown.contains("unknown provider"), "{shown}");

    assert_eq!(config(site.home()), before, "a refused member wrote config.toml");
    assert_eq!(config(site.home()), config(twin.path()), "config.toml differs from the CLI's");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_two_call_test_asks_to_confirm_first() {
    let mock = start_mock().await;
    let site = Site::start_on(home_at(&mock)).await;
    // The question the CLI prints, from the plan the running server makes for the same target.
    let ctx = Ctx { home: site.d.home(), engine: Some(site.d.engine.clone()) };
    let plan = model_tests::plan(&ctx, Some("u"), None).expect("the plan");
    assert!(plan.needs_confirm(), "{plan:?}");
    let question = plan.question();

    let fields = [("target", "u".to_owned()), ("_window", "test".to_owned())];
    let r = site.post("test/run", "/combo?target=u", &fields).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let location = r.headers()[LOCATION].to_str().unwrap().to_owned();
    assert!(location.contains("done=") && location.contains("window=test"), "{location}");

    let (status, page) = site.get(&location).await;
    assert_eq!(status, StatusCode::OK, "{location}");
    let (refused, shown) = notice(&page).expect("the refusal is shown");
    assert!(refused, "{shown}");
    assert_eq!(shown, question);
    assert!(question.contains("billed calls") && question.contains("Continue?"), "{question}");
    // The window asks again, now confirmed, so the next post runs the test.
    let form = form_of(&page, RUN);
    assert!(form.contains("name=\"target\" value=\"u\""), "{form}");
    assert!(form.contains("name=\"confirmed\" value=\"on\""), "the window does not confirm: {form}");
    assert!(mock.received().is_empty(), "a refused test called the upstream");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_started_test_shows_started_and_after_it_ends_the_verdict_matches_verdicts_json() {
    let mock = start_mock().await;
    let site = Site::start_on(home_at(&mock)).await;
    let back = "/combo?window=test&target=u";
    let fields = [("target", "u".to_owned()), ("confirmed", "on".to_owned())];
    let r = site.post("test/run", back, &fields).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let location = r.headers()[LOCATION].to_str().unwrap().to_owned();
    assert!(location.starts_with(back) && location.contains("task="), "{location}");

    let (status, first) = site.get(&location).await;
    assert_eq!(status, StatusCode::OK, "{location}");
    assert!(text_of(&first).contains("started at"), "the panel says when it started: {}", text_of(&first));

    // Poll the panel a bounded number of times; its refresh stops once the run has ended.
    let mut page = first;
    for _ in 0..200 {
        if !page.contains("http-equiv=\"refresh\"") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        page = site.get(&location).await.1;
    }
    assert!(!page.contains("http-equiv=\"refresh\""), "the test did not end: {}", text_of(&page));
    let (refused, shown) = notice(&page).expect("the task's result");
    assert!(!refused, "{shown}");
    assert_eq!(shown, "2 pairs: 2 pass, 0 broken, 0 unknown, 0 skipped");
    assert_eq!(mock.received().len(), 2, "one call per pair");

    let log = text_of(&site.get("/console-log").await.1);
    assert!(log.contains("started"), "the change log shows the run as started: {log}");

    // `verdicts --json` with the running server: the verdicts the run just saved.
    let fetched = fetch_in_process(&site.d.engine, verdicts::NEEDS, &json!({})).await;
    let v = verdicts::build(&site.d.home(), &json!({}), &fetched).expect("the verdicts").json;
    let rows = v["verdicts"].as_array().unwrap();
    let mut accounts: Vec<&str> = rows.iter().map(|r| r["account"].as_str().unwrap()).collect();
    accounts.sort();
    assert_eq!(accounts, ["backup", "main"], "{v}");
    assert!(rows.iter().all(|r| r["provider"] == "anthropic" && r["model"] == "m1" && r["state"] == "pass"), "{v}");
}
