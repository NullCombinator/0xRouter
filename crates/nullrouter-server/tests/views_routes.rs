//! The read model gives one answer by both routes (spec 008, FR-003, SC-002, research R5): on each
//! fixture home with a real server, every view is built from the socket's answers (the CLI route)
//! and from `operator::handle`'s (the in-server route), and `json` and `extra` must be equal.
//! `routing`'s `now` is the one field that changes between two reads; it is compared within the
//! seconds that elapsed.

use std::sync::Arc;

mod common;

use common::view_cases as cases;
use nullrouter_engine::clock::parse_rfc3339;
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::homes;
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator;
use nullrouter_server::views;
use serde_json::Value;

/// Equal, except `now` (seconds since the other read) within `slack_s`.
fn same(name: &str, a: &Value, b: &Value) {
    let (mut a, mut b) = (a.clone(), b.clone());
    if let (Some(x), Some(y)) = (a["now"].as_str().and_then(parse_rfc3339), b["now"].as_str().and_then(parse_rfc3339)) {
        let gap = y.duration_since(x).unwrap_or_else(|e| e.duration()).as_secs();
        assert!(gap <= 5, "{name}: the two reads were {gap} s apart");
        a["now"] = Value::Null;
        b["now"] = Value::Null;
    }
    assert_eq!(a, b, "{name}");
}

#[tokio::test(flavor = "multi_thread")]
async fn every_view_is_the_same_by_both_routes() {
    for fixture in homes::ALL.iter().filter(|f| f.can_serve) {
        let dir = (fixture.build)();
        let home = OperatorHome::new(dir.path());
        let (engine, _) = Engine::open(home.clone()).unwrap();
        let engine = Arc::new(engine);
        let listener = operator::bind(&home).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(operator::serve(engine.clone(), listener, async move {
            let _ = stopped.await;
        }));
        if fixture.name == "full" {
            homes::unfinished(dir.path());
        }
        for (name, needs, args, build) in cases() {
            let by_socket = {
                let (home, args) = (home.clone(), args.clone());
                tokio::task::spawn_blocking(move || {
                    let live = views::fetch_socket(&home, needs, &args);
                    build(&home, &args, &live)
                })
                .await
                .unwrap()
            };
            let by_server = {
                let (home, args2) = (home.clone(), args.clone());
                views::run_in_process(&engine, needs, &args, move |live| build(&home, &args2, live)).await
            };
            let label = format!("{} / {name}", fixture.name);
            match (by_socket, by_server) {
                (Ok(a), Ok(b)) => {
                    same(&label, &a.json, &b.json);
                    same(&label, &a.extra, &b.extra);
                }
                (a, b) => assert_eq!(a, b, "{label}"),
            }
        }
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}

/// No view reads the engine's loaded snapshot (spec 008 Edge Cases): after an edit the server has
/// not applied, the in-server route still answers from the files.
#[tokio::test(flavor = "multi_thread")]
async fn an_unapplied_edit_shows_in_the_in_server_answer() {
    let dir = homes::full();
    let home = OperatorHome::new(dir.path());
    let (engine, _) = Engine::open(home.clone()).unwrap();
    let engine = Arc::new(engine);

    let config = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        config + "\n[[unified_model]]\nname = \"fresh\"\nmembers = [{ provider = \"xai\", model = \"grok-4\" }]\n",
    )
    .unwrap();
    let accounts = std::fs::read_to_string(dir.path().join("accounts.toml")).unwrap();
    std::fs::write(
        dir.path().join("accounts.toml"),
        accounts
            + "\n[[account]]\nprovider = \"anthropic\"\nname = \"added\"\nsecret = \"sk-added-0000ZZZZ\"\norder = 9\n",
    )
    .unwrap();
    assert!(engine.snapshot().registry.resolve("fresh").is_err(), "the server has not applied the edit");

    let resolve = |home: OperatorHome| {
        views::run_in_process(&engine, views::resolve::NEEDS, &Value::Null, move |live| {
            views::resolve::build(&home, &serde_json::json!({"target": "fresh"}), live)
        })
    };
    assert_eq!(resolve(home.clone()).await.unwrap().json["kind"], "unified");
    let accounts = views::run_in_process(&engine, views::accounts::NEEDS, &Value::Null, {
        let home = home.clone();
        move |live| views::accounts::build(&home, &serde_json::json!({"provider": null}), live)
    })
    .await
    .unwrap();
    assert!(accounts.json.as_array().unwrap().iter().any(|a| a["name"] == "added"), "{}", accounts.json);
}
