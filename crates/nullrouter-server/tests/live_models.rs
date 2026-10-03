//! Live models are served, not only listed (spec 005 T100; FR-008, US1 scenario 8; research
//! R14). The plugin is the bundled grok-cli file with its hosts pointed at the mock, its live
//! list typed from each entry's `kind` (default text), and uncatalogued models turned off, so
//! an id resolves only from the static `[[models]]` or the live list.

mod common;

use std::sync::{Arc, Mutex};

use common::{STYLES, Server, bundled_at_mock, reply_by_wire, signin_server, style_request};
use nullrouter_engine::testkit::{Received, Step};
use serde_json::{Value, json};

const CONFIG: &str = r#"[plugin_decisions]
"grok-cli" = "replace"
[provider.grok-cli]
allow_uncatalogued_models = false
[[unified_model]]
name = "live"
members = [{ provider = "grok-cli", model = "grok-live" }]
"#;

const MODELS: &str = "/v1/models";

/// The server and the live list the mock answers with.
async fn server() -> (Server, Arc<Mutex<Step>>) {
    let s = signin_server(
        |m| vec![("grok-cli", bundled_at_mock(m, "grok-cli").replace("type = \"text\"", "type = \"kind\""))],
        &[("grok-cli", "work")],
        CONFIG,
    )
    .await;
    let list = Arc::new(Mutex::new(Step::json(500, json!({"error": "down"}))));
    let l = list.clone();
    s.mock.respond(move |r: &Received| {
        if r.path_and_query.split('?').next() == Some(MODELS) {
            return l.lock().unwrap().clone();
        }
        reply_by_wire(r)
    });
    (s, list)
}

fn good_list() -> Step {
    Step::json(
        200,
        json!({"data": [
            {"id": "grok-build"},
            {"id": "grok-live", "display_name": "Grok Live"},
            {"id": "grok-img", "kind": "image"},
        ]}),
    )
}

/// The ids and types a client sees on the model list of `shape`.
async fn listed(s: &Server, shape: &str) -> Vec<(String, String)> {
    let path = if shape == "gemini" { "/v1beta/models" } else { "/v1/models" };
    let mut req =
        reqwest::Client::new().get(format!("{}{path}", s.base)).bearer_auth(&s.key).header("x-goog-api-key", &s.key);
    if shape == "anthropic-messages" {
        req = req.header("x-api-key", &s.key).header("anthropic-version", "2023-06-01");
    }
    let v: Value = serde_json::from_slice(&req.send().await.unwrap().bytes().await.unwrap()).unwrap();
    let (list, key) = if shape == "gemini" { (&v["models"], "name") } else { (&v["data"], "id") };
    list.as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let id = e[key].as_str().unwrap().trim_start_matches("models/").to_owned();
            (id, e["nullrouter"]["type"].as_str().unwrap().to_owned())
        })
        .collect()
}

/// The inference requests the mock received.
fn sent(s: &Server) -> Vec<Received> {
    s.mock.received().into_iter().filter(|r| r.path_and_query.split('?').next() != Some(MODELS)).collect()
}

async fn status_and_body(req: reqwest::RequestBuilder) -> (u16, Value) {
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    (status, serde_json::from_slice(&r.bytes().await.unwrap()).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_live_only_model_is_listed_and_served_in_every_style() {
    let (s, list) = server().await;
    // Before any good read, the live-only id is in neither list.
    assert!(s.engine.fetch_live_models("grok-cli").await.is_err());
    let (status, _) = status_and_body(style_request(&s.base, &s.key, "openai-chat", "grok-cli/grok-live")).await;
    assert_eq!(status, 404, "not found before the live list names it");

    *list.lock().unwrap() = good_list();
    assert_eq!(s.engine.fetch_live_models("grok-cli").await.unwrap(), 3);
    for shape in ["openai-chat", "anthropic-messages", "gemini"] {
        let l = listed(&s, shape).await;
        assert!(l.contains(&("grok-cli/grok-live".into(), "text".into())), "{shape}: {l:?}");
        assert!(!l.iter().any(|(i, _)| i == "grok-cli/grok-img"), "{shape}: no image endpoint, so not listed");
    }

    for style in STYLES {
        let before = sent(&s).len();
        let (status, body) = status_and_body(style_request(&s.base, &s.key, style, "grok-cli/grok-live")).await;
        assert_eq!(status, 200, "{style}: {body}");
        let up = &sent(&s)[before..];
        assert_eq!(up.len(), 1, "{style}");
        assert!(up[0].path_and_query.ends_with("/v1/responses"), "{style}: {}", up[0].path_and_query);
        assert_eq!(up[0].json()["model"], "grok-live", "{style}: the upstream id is the live id");
        assert_eq!(up[0].headers["x-grok-model-override"], "grok-live", "{style}");
    }

    // A unified model naming the live id.
    let before = sent(&s).len();
    let (status, body) = status_and_body(style_request(&s.base, &s.key, "openai-chat", "live")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(sent(&s)[before..][0].json()["model"], "grok-live");

    // The live list's type holds: an image model isn't served on a text route.
    let (status, body) = status_and_body(style_request(&s.base, &s.key, "openai-chat", "grok-cli/grok-img")).await;
    assert_eq!(status, 400, "{body}");
    assert!(body.to_string().contains("image"), "{body}");
}

#[tokio::test]
async fn after_a_failed_refresh_the_last_good_list_still_resolves() {
    let (s, list) = server().await;
    *list.lock().unwrap() = good_list();
    s.engine.fetch_live_models("grok-cli").await.unwrap();
    *list.lock().unwrap() = Step::json(500, json!({"error": "down"}));
    assert!(s.engine.fetch_live_models("grok-cli").await.is_err());

    let (status, body) = status_and_body(style_request(&s.base, &s.key, "openai-chat", "grok-cli/grok-live")).await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = status_and_body(style_request(&s.base, &s.key, "anthropic-messages", "live")).await;
    assert_eq!(status, 200, "{body}");
    assert!(sent(&s).iter().all(|r| r.json()["model"] == "grok-live"));
}

#[tokio::test]
async fn an_id_in_neither_list_is_refused_in_the_clients_style() {
    let (s, list) = server().await;
    *list.lock().unwrap() = good_list();
    s.engine.fetch_live_models("grok-cli").await.unwrap();
    for style in STYLES {
        let (status, body) = status_and_body(style_request(&s.base, &s.key, style, "grok-cli/grok-nope")).await;
        assert_eq!(status, 404, "{style}: {body}");
        match style {
            "anthropic-messages" => assert_eq!(body["type"], "error", "{body}"),
            _ => assert!(body["error"].is_object(), "{style}: {body}"),
        }
        assert!(body.to_string().contains("grok-nope"), "{style}: {body}");
    }
    // A static id still resolves.
    let (status, body) = status_and_body(style_request(&s.base, &s.key, "openai-chat", "grok-cli/grok-4.5")).await;
    assert_eq!(status, 200, "{body}");
    assert!(sent(&s).iter().all(|r| r.json()["model"] != "grok-nope"), "nothing sent for the unknown id");
}
