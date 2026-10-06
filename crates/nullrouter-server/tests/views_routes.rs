//! The read model gives one answer by both routes (spec 008, FR-003, SC-002, research R5): on each
//! fixture home with a real server, every view is built from the socket's answers (the CLI route)
//! and from `operator::handle`'s (the in-server route), and `json` and `extra` must be equal.
//! `routing`'s `now` is the one field that changes between two reads; it is compared within the
//! seconds that elapsed.

use std::sync::Arc;

use nullrouter_engine::clock::parse_rfc3339;
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::homes;
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator;
use nullrouter_server::views::{self, Live, View, ViewError};
use serde_json::{Value, json};

type Build = fn(&OperatorHome, &Value, &Live) -> Result<View, ViewError>;

/// Every view moved so far, with the arguments it is exercised with.
fn cases() -> Vec<(&'static str, &'static [&'static str], Value, Build)> {
    use views::*;
    let r = homes::record_id;
    let mut v: Vec<(&'static str, &'static [&'static str], Value, Build)> = vec![
        ("keys", keys::NEEDS, json!({}), keys::build),
        ("accounts", accounts::NEEDS, json!({"provider": null}), accounts::build),
        ("accounts xai", accounts::NEEDS, json!({"provider": "xai"}), accounts::build),
        ("quota", quota::NEEDS, json!({"provider": null, "name": null}), quota::build),
        ("quota xai work", quota::NEEDS, json!({"provider": "xai", "name": "work"}), quota::build),
        (
            "quota history",
            quota::HISTORY_NEEDS,
            json!({"provider": "xai", "name": "work", "since": null, "limit": null}),
            quota::history,
        ),
        ("routing", routing::NEEDS, json!({"target": null}), routing::build),
        ("routing mixed", routing::NEEDS, json!({"target": "mixed"}), routing::build),
        ("records", records::NEEDS, json!({}), records::build),
        ("records filtered", records::NEEDS, json!({"agent": "ak_fixture1", "limit": 2}), records::build),
        ("providers", providers::NEEDS, json!({"capability": null}), providers::build),
        ("providers tts", providers::NEEDS, json!({"capability": "tts"}), providers::build),
        ("model", model::NEEDS, json!({"provider": "xai", "model": "grok-4"}), model::build),
        ("model unknown", model::NEEDS, json!({"provider": "nope", "model": "m"}), model::build),
        ("plugins", plugins::NEEDS, json!({"community": false}), plugins::build),
        ("plugins community", plugins::NEEDS, json!({"community": true}), plugins::build),
        ("check", check::NEEDS, json!({}), check::build),
        ("resolve direct", resolve::NEEDS, json!({"target": "grok-cli/grok-build"}), resolve::build),
        ("resolve unified", resolve::NEEDS, json!({"target": "mixed"}), resolve::build),
        ("resolve unknown", resolve::NEEDS, json!({"target": "nope"}), resolve::build),
    ];
    for n in [1, 2, 5, 6] {
        v.push(("record", records::RECORD_NEEDS, json!({"id": r(n)}), records::record));
    }
    v.push(("record unknown", records::RECORD_NEEDS, json!({"id": "rq_missing"}), records::record));
    v
}

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
