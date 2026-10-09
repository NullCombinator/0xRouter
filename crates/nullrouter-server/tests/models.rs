//! Model lists over HTTP (T110, US6-1, US6-2): the OpenAI, Anthropic and Gemini shapes list
//! every unified model and every direct model of every type on providers with an account,
//! each with its type. A provider with no account is absent; `get_model` takes ids with `/`.

mod common;

use common::{Server, server_with};
use nullrouter_engine::testkit::MockUpstream;
use nullrouter_server::operator;
use serde_json::{Value, json};

fn typed(mock: &MockUpstream) -> (&'static str, String) {
    let toml = format!(
        r#"schema = 2
id = "typed"
category = "apikey"
[auth]
kind = "apikey"
[endpoints.text]
url = "{chat}"
wire = "openai-chat"
[endpoints.embeddings]
url = "{emb}"
wire = "openai-chat"
[[models]]
id = "vendor/chat-1"
[[models]]
id = "emb"
kind = "embedding"
"#,
        chat = mock.url("/typed/chat/completions"),
        emb = mock.url("/typed/embeddings"),
    );
    ("typed", toml)
}

/// The server, plus a provider with no account, two unified models and a combo over each (spec
/// 011 US5 scenario 7), loaded by a reload.
async fn start() -> Server {
    let s = server_with(|mock| vec![typed(mock)]).await;
    let lonely = format!(
        "schema = 2\nid = \"lonely\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"alone\"\n",
        s.mock.url("/lonely/chat/completions")
    );
    std::fs::write(s.home().join("plugins/lonely.toml"), lonely).unwrap();
    let config = "allow_private_endpoints = true\n\
        [[unified_model]]\nname = \"smart\"\nmembers = [{ provider = \"mockco\", model = \"m1\" }, { provider = \"typed\", model = \"vendor/chat-1\" }]\n\
        [[unified_model]]\nname = \"vectors\"\nkind = \"embedding\"\nmembers = [{ provider = \"typed\", model = \"emb\" }]\n\
        [[combo]]\nname = \"coder\"\nmembers = [\"smart\"]\n\
        [[combo]]\nname = \"search\"\nmembers = [\"vectors\"]\n";
    std::fs::write(s.home().join("config.toml"), config).unwrap();
    let r = operator::handle(&s.engine, &json!({"op": "reload"})).await;
    assert_eq!(r["ok"], true, "{r}");
    s
}

async fn get(s: &Server, path: &str, anthropic: bool) -> (u16, Value) {
    let mut req =
        reqwest::Client::new().get(format!("{}{path}", s.base)).bearer_auth(&s.key).header("x-goog-api-key", &s.key);
    if anthropic {
        req = req.header("x-api-key", &s.key).header("anthropic-version", "2023-06-01");
    }
    let r = req.send().await.unwrap();
    (r.status().as_u16(), serde_json::from_slice::<Value>(&r.bytes().await.unwrap()).unwrap())
}

/// The expected listing: id → type.
const LISTED: [(&str, &str); 7] = [
    ("smart", "text"),
    ("vectors", "embeddings"),
    ("coder", "text"),
    ("search", "embeddings"),
    ("mockco/m1", "text"),
    ("typed/vendor/chat-1", "text"),
    ("typed/emb", "embeddings"),
];

fn check(what: &str, entries: &[Value], id: impl Fn(&Value) -> String) {
    let got: Vec<(String, String)> =
        entries.iter().map(|e| (id(e), e["nullrouter"]["type"].as_str().unwrap_or_default().to_owned())).collect();
    for (want, ty) in LISTED {
        assert!(got.iter().any(|(i, t)| i == want && t == ty), "{what}: {want} ({ty}) missing from {got:?}");
    }
    for (i, _) in &got {
        assert!(!i.starts_with("lonely/"), "{what}: a provider with no account is listed: {i}");
        assert!(
            !i.starts_with("anthropic/") && !i.starts_with("openrouter/"),
            "{what}: bundled provider with no account: {i}"
        );
    }
}

#[tokio::test]
async fn every_style_lists_what_the_client_can_reach_in_its_own_shape() {
    let s = start().await;

    let (status, body) = get(&s, "/v1/models", false).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["object"], "list");
    let data = body["data"].as_array().unwrap();
    assert!(data.iter().all(|e| e["object"] == "model" && e["owned_by"].is_string()), "{body}");
    check("openai", data, |e| e["id"].as_str().unwrap().to_owned());

    let (status, body) = get(&s, "/v1/models", true).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["has_more"], false);
    let data = body["data"].as_array().unwrap();
    assert!(data.iter().all(|e| e["type"] == "model" && e["display_name"].is_string()), "{body}");
    check("anthropic", data, |e| e["id"].as_str().unwrap().to_owned());

    let (status, body) = get(&s, "/v1beta/models", false).await;
    assert_eq!(status, 200, "{body}");
    let models = body["models"].as_array().unwrap();
    check("gemini", models, |e| e["name"].as_str().unwrap().strip_prefix("models/").unwrap().to_owned());
    let methods = |id: &str| {
        let e = models.iter().find(|e| e["name"] == format!("models/{id}")).unwrap();
        e["supportedGenerationMethods"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert!(methods("typed/vendor/chat-1").contains(&"generateContent".to_owned()));
    assert!(methods("typed/emb").contains(&"embedContent".to_owned()));
}

#[tokio::test]
async fn get_model_takes_ids_with_slashes() {
    let s = start().await;
    let (status, body) = get(&s, "/v1/models/typed/vendor/chat-1", false).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["id"], "typed/vendor/chat-1");
    assert_eq!(body["nullrouter"]["type"], "text");

    let (status, body) = get(&s, "/v1/models/typed/emb", true).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["type"], "model");
    assert_eq!(body["nullrouter"]["type"], "embeddings");

    let (status, body) = get(&s, "/v1beta/models/smart", false).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["name"], "models/smart");

    let (status, _) = get(&s, "/v1/models/lonely/alone", false).await;
    assert_eq!(status, 404, "a provider with no account has no models to get");
}

/// Spec 011 T025, clarify Q2: a BROKEN pair stays listed; the list says what a client may ask
/// for, and the error at request time says why it failed.
#[tokio::test]
async fn a_target_broken_on_every_pair_is_still_listed() {
    use nullrouter_engine::verdict::{Basis, Pair, Rejection, Source, State, Verdict};
    let s = start().await;
    for (provider, model) in [("mockco", "m1"), ("typed", "vendor/chat-1"), ("typed", "emb")] {
        let v = Verdict {
            state: State::Broken,
            reason: "404: model does not exist".into(),
            rejection: Some(Rejection::ModelNotFound),
            source: Source::Operator,
            at: std::time::SystemTime::now(),
            record: None,
            step: None,
            next: None,
            basis: Basis::default(),
            note: None,
        };
        s.engine.verdicts.set(Pair::new(provider, "main", model), v);
    }
    let (_, body) = get(&s, "/v1/models", false).await;
    check("openai", body["data"].as_array().unwrap(), |e| e["id"].as_str().unwrap().to_owned());
    let (_, body) = get(&s, "/v1/models", true).await;
    check("anthropic", body["data"].as_array().unwrap(), |e| e["id"].as_str().unwrap().to_owned());
    let (_, body) = get(&s, "/v1beta/models", false).await;
    check("gemini", body["models"].as_array().unwrap(), |e| {
        e["name"].as_str().unwrap().strip_prefix("models/").unwrap().to_owned()
    });
}
