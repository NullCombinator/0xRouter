//! Non-text types over HTTP (T076, US3-1 to US3-3): embeddings, image, speech,
//! transcription and video jobs, through the OpenAI and Gemini routes, against scripted
//! providers. `mediaco` speaks openai-chat for every type (openrouter's shape); `voiceco`
//! declares inline speech and transcription bodies (elevenlabs's shape).

mod common;

use common::{SECRET, Server, server_with};
use serde_json::{Value, json};
use zerorouter_engine::records::{Outcome, RequestRecord};
use zerorouter_engine::testkit::{MockUpstream, Step};
use zerorouter_registry::schema::ModelType;
use zerorouter_server::relay::REQUEST_ID;
use zerorouter_wire::primitives::body;

fn mediaco(mock: &MockUpstream) -> (&'static str, String) {
    let toml = format!(
        r#"schema = 2
id = "mediaco"
category = "apikey"
[auth]
kind = "apikey"
[endpoints.embeddings]
url = "{emb}"
wire = "openai-chat"
[endpoints.image]
url = "{img}"
wire = "openai-chat"
[endpoints.tts]
url = "{tts}"
wire = "openai-chat"
voices = ["alloy", "nova"]
[endpoints.video]
url = "{vid}"
wire = "openai-chat"
[[models]]
id = "emb"
kind = "embedding"
[[models]]
id = "img"
kind = "image"
[[models]]
id = "say"
kind = "tts"
[[models]]
id = "vid"
kind = "video"
"#,
        emb = mock.url("/media/embeddings"),
        img = mock.url("/media/images"),
        tts = mock.url("/media/audio/speech"),
        vid = mock.url("/media/videos"),
    );
    ("mediaco", toml)
}

fn voiceco(mock: &MockUpstream) -> (&'static str, String) {
    let toml = format!(
        r#"schema = 2
id = "voiceco"
category = "apikey"
[auth]
kind = "apikey"
header = "xi-api-key"
scheme = "raw"
[endpoints.tts]
url = "{tts}"
body = {{ text = "{{input.text}}", model_id = "{{model.upstream_id}}" }}
response = {{}}
voices = ["v-default"]
[endpoints.stt]
url = "{stt}"
encoding = "multipart"
body = {{ file = "{{input.audio}}", model_id = "{{model.upstream_id}}", language_code = "{{input.language?}}" }}
response = {{ text = "text", language = "language_code" }}
[[models]]
id = "flash"
kind = "tts"
[[models]]
id = "scribe_v2"
kind = "stt"
"#,
        tts = mock.url("/voice/text-to-speech/{voice}/stream"),
        stt = mock.url("/voice/speech-to-text"),
    );
    ("voiceco", toml)
}

async fn types_server() -> Server {
    server_with(|m| vec![mediaco(m), voiceco(m)]).await
}

fn post(s: &Server, path: &str, body: &Value) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(format!("{}{path}", s.base))
        .bearer_auth(&s.key)
        .header("content-type", "application/json")
        .body(body.to_string())
}

fn get(s: &Server, url: &str) -> reqwest::RequestBuilder {
    let url = if url.starts_with("http") { url.to_owned() } else { format!("{}{url}", s.base) };
    reqwest::Client::new().get(url).bearer_auth(&s.key)
}

async fn json(r: reqwest::Response) -> Value {
    serde_json::from_slice(&r.bytes().await.unwrap()).unwrap()
}

fn record(s: &Server, r: &reqwest::Response) -> RequestRecord {
    let id = r.headers()[REQUEST_ID].to_str().unwrap();
    s.engine.records.get(id).unwrap()
}

fn embeddings_answer() -> Step {
    Step::json(
        200,
        json!({"object": "list", "data": [{"object": "embedding", "index": 0, "embedding": [0.25, -0.5]}], "model": "emb", "usage": {"prompt_tokens": 3, "total_tokens": 3}}),
    )
}

#[tokio::test]
async fn openai_embeddings() {
    let s = types_server().await;
    s.mock.on("/media/embeddings", [embeddings_answer()]);
    let r = post(&s, "/v1/embeddings", &json!({"model": "mediaco/emb", "input": "hello"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let rec = record(&s, &r);
    let body: Value = json(r).await;
    assert_eq!(body["data"][0]["embedding"], json!([0.25, -0.5]));
    let sent = &s.mock.received()[0];
    assert_eq!(sent.json()["model"], "emb", "the provider's model id");
    assert_eq!(sent.json()["input"], "hello");
    assert_eq!(sent.headers["authorization"], format!("Bearer {SECRET}"));
    assert_eq!(rec.model_type, Some(ModelType::Embeddings));
    assert_eq!(rec.outcome, Outcome::Succeeded);
    assert_eq!(rec.usage.unwrap().input, Some(3));
}

#[tokio::test]
async fn openai_image_b64_json() {
    let s = types_server().await;
    s.mock.on(
        "/media/images",
        [Step::json(
            200,
            json!({"created": 1, "data": [{"b64_json": "aGk="}], "usage": {"input_tokens": 4, "output_tokens": 10}}),
        )],
    );
    let r = post(
        &s,
        "/v1/images/generations",
        &json!({"model": "mediaco/img", "prompt": "a cat", "response_format": "b64_json"}),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(r.status(), 200);
    let rec = record(&s, &r);
    let body: Value = json(r).await;
    assert_eq!(body["data"][0]["b64_json"], "aGk=");
    assert_eq!(s.mock.received()[0].json()["prompt"], "a cat");
    assert_eq!(rec.model_type, Some(ModelType::Image));
    let usage = rec.usage.unwrap();
    assert_eq!((usage.input, usage.output), (Some(4), Some(10)));
}

#[tokio::test]
async fn openai_speech_is_relayed_as_audio() {
    let s = types_server().await;
    s.mock.on(
        "/media/audio/speech",
        [Step::binary("audio/mpeg", &b"ID3-audio"[..]), Step::binary("audio/mpeg", &b"ID3-audio"[..])],
    );
    let r = post(&s, "/v1/audio/speech", &json!({"model": "mediaco/say", "input": "hi", "voice": "nova"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "audio/mpeg");
    let rec = record(&s, &r);
    assert_eq!(&r.bytes().await.unwrap()[..], b"ID3-audio");
    assert_eq!(s.mock.received()[0].json()["voice"], "nova");
    assert_eq!(rec.model_type, Some(ModelType::Tts));
    assert_eq!(rec.outcome, Outcome::Succeeded);

    // 9router's `provider/model/voice` form.
    let r = post(&s, "/v1/audio/speech", &json!({"model": "mediaco/say/nova", "input": "hi"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let sent = s.mock.received()[1].json();
    assert_eq!((sent["model"].as_str(), sent["voice"].as_str()), (Some("say"), Some("nova")));
}

#[tokio::test]
async fn inline_speech_streams_binary_audio() {
    let s = types_server().await;
    let frames = vec![bytes::Bytes::from_static(b"chunk-1|"), bytes::Bytes::from_static(b"chunk-2")];
    let stream = Step::Stream {
        status: 200,
        headers: vec![("content-type".into(), "audio/mpeg".into())],
        frames,
        every: std::time::Duration::from_millis(20),
        cut: false,
    };
    s.mock.on("/voice/text-to-speech", [stream]);
    let r = post(&s, "/v1/audio/speech", &json!({"model": "voiceco/flash", "input": "hello"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "audio/mpeg");
    let rec_id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    assert_eq!(&r.bytes().await.unwrap()[..], b"chunk-1|chunk-2");
    let sent = &s.mock.received()[0];
    assert_eq!(sent.path_and_query, "/voice/text-to-speech/v-default/stream", "the endpoint's first voice");
    assert_eq!(sent.json(), json!({"text": "hello", "model_id": "flash"}));
    assert_eq!(sent.headers["xi-api-key"], SECRET);
    let rec = s.engine.records.get(&rec_id).unwrap();
    assert_eq!(rec.model_type, Some(ModelType::Tts));
}

#[tokio::test]
async fn inline_transcription_is_multipart() {
    let s = types_server().await;
    s.mock.on("/voice/speech-to-text", [Step::json(200, json!({"text": "hello world", "language_code": "en"}))]);
    let (form, ctype) = body::encode_multipart(
        &json!({"file": body::file(b"RIFF-wave", Some("a.wav"), Some("audio/wav")), "model": "voiceco/scribe_v2"}),
    );
    let r = reqwest::Client::new()
        .post(format!("{}/v1/audio/transcriptions", s.base))
        .bearer_auth(&s.key)
        .header("content-type", ctype)
        .body(form)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let rec = record(&s, &r);
    let body: Value = json(r).await;
    assert_eq!(body["text"], "hello world");
    assert_eq!(body["language"], "en");
    let sent = &s.mock.received()[0];
    let ctype = sent.headers["content-type"].to_str().unwrap();
    let parts = body::parse_multipart(&sent.body, body::boundary(ctype).unwrap()).unwrap();
    assert_eq!(parts["model_id"], "scribe_v2");
    assert_eq!(body::bytes_of(&parts["file"]).unwrap(), b"RIFF-wave");
    assert_eq!(rec.model_type, Some(ModelType::Stt));
}

#[tokio::test]
async fn openai_video_job_submit_poll_and_content() {
    let s = types_server().await;
    s.mock.on("/media/videos/up_v1/content", [Step::binary("video/mp4", &b"MP4-bytes"[..])]);
    s.mock.on(
        "/media/videos/up_v1",
        [
            Step::json(200, json!({"id": "up_v1", "object": "video", "status": "in_progress"})),
            Step::json(200, json!({"id": "up_v1", "object": "video", "status": "completed"})),
        ],
    );
    s.mock.on("/media/videos", [Step::accepted(json!({"id": "up_v1", "object": "video", "status": "queued"}))]);

    let r = post(&s, "/v1/videos", &json!({"model": "mediaco/vid", "prompt": "a cat"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let rec_id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let job: Value = json(r).await;
    let vj = job["id"].as_str().unwrap().to_owned();
    assert!(vj.starts_with("vj_"), "the provider's id stays hidden: {job}");
    assert_eq!(job["status"], "queued");
    assert_eq!(job["model"], "mediaco/vid");
    let rec = s.engine.records.get(&rec_id).unwrap();
    assert_eq!(rec.outcome, Outcome::InProgress, "a job's record stays open");
    assert_eq!(rec.job.as_ref().unwrap().upstream_id, "up_v1");

    let poll = get(&s, &format!("/v1/videos/{vj}")).send().await.unwrap();
    assert_eq!(poll.headers()[REQUEST_ID].to_str().unwrap(), rec_id, "polls belong to the submit's record");
    assert_eq!(json(poll).await["status"], "in_progress");
    let poll: Value = json(get(&s, &format!("/v1/videos/{vj}")).send().await.unwrap()).await;
    assert_eq!(poll["status"], "completed");
    assert_eq!(poll["id"], vj.as_str());

    let content = get(&s, &format!("/v1/videos/{vj}/content")).send().await.unwrap();
    assert_eq!(content.status(), 200);
    assert_eq!(content.headers()["content-type"], "video/mp4");
    assert_eq!(&content.bytes().await.unwrap()[..], b"MP4-bytes");
    let paths: Vec<String> = s.mock.received().into_iter().map(|r| r.path_and_query).collect();
    assert_eq!(paths, ["/media/videos", "/media/videos/up_v1", "/media/videos/up_v1", "/media/videos/up_v1/content"]);
    assert!(s.mock.received().iter().all(|r| r.headers["authorization"] == format!("Bearer {SECRET}")));
    for _ in 0..50 {
        if s.engine.records.get(&rec_id).unwrap().outcome == Outcome::Succeeded {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(s.engine.records.get(&rec_id).unwrap().outcome, Outcome::Succeeded, "the content read to its end");

    let unknown = get(&s, "/v1/videos/vj_nope").send().await.unwrap();
    assert_eq!(unknown.status(), 404);
}

fn gemini_post(s: &Server, path: &str, body: &Value) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(format!("{}{path}", s.base))
        .header("x-goog-api-key", &s.key)
        .header("content-type", "application/json")
        .body(body.to_string())
}

#[tokio::test]
async fn gemini_embed_content_and_batch() {
    let s = types_server().await;
    s.mock.on("/media/embeddings", [embeddings_answer()]);
    let r =
        gemini_post(&s, "/v1beta/models/mediaco/emb:embedContent", &json!({"content": {"parts": [{"text": "hello"}]}}))
            .send()
            .await
            .unwrap();
    assert_eq!(r.status(), 200);
    let rec = record(&s, &r);
    let body: Value = json(r).await;
    assert_eq!(body, json!({"embedding": {"values": [0.25, -0.5]}}));
    let sent = s.mock.received()[0].json();
    assert_eq!((sent["model"].as_str(), sent["input"].as_str()), (Some("emb"), Some("hello")), "{sent}");
    assert_eq!(rec.model_type, Some(ModelType::Embeddings));
    assert_eq!(rec.style, "gemini");

    s.mock.on(
        "/media/embeddings",
        [Step::json(
            200,
            json!({"object": "list", "data": [{"object": "embedding", "index": 0, "embedding": [1.0]}, {"object": "embedding", "index": 1, "embedding": [2.0]}], "model": "emb", "usage": {"prompt_tokens": 2, "total_tokens": 2}}),
        )],
    );
    let batch = json!({"requests": [{"model": "models/emb", "content": {"parts": [{"text": "a"}]}}, {"model": "models/emb", "content": {"parts": [{"text": "b"}]}}]});
    let r = gemini_post(&s, "/v1beta/models/mediaco/emb:batchEmbedContents", &batch).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = json(r).await;
    assert_eq!(body, json!({"embeddings": [{"values": [1.0]}, {"values": [2.0]}]}));
    assert_eq!(s.mock.received()[1].json()["input"], json!(["a", "b"]));
}

#[tokio::test]
async fn gemini_image_and_speech_by_response_modality() {
    let s = types_server().await;
    s.mock.on("/media/images", [Step::json(200, json!({"created": 1, "data": [{"b64_json": "aGk="}]}))]);
    s.mock.on("/media/audio/speech", [Step::binary("audio/mpeg", &b"ID3-audio"[..])]);
    let image =
        json!({"contents": [{"parts": [{"text": "a cat"}]}], "generationConfig": {"responseModalities": ["IMAGE"]}});
    let r = gemini_post(&s, "/v1beta/models/mediaco/img:generateContent", &image).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(record(&s, &r).model_type, Some(ModelType::Image));
    let body: Value = json(r).await;
    assert_eq!(body["candidates"][0]["content"]["parts"][0]["inlineData"]["data"], "aGk=");
    assert_eq!(s.mock.received()[0].json()["prompt"], "a cat");

    let speech = json!({"contents": [{"parts": [{"text": "hello"}]}], "generationConfig": {"responseModalities": ["AUDIO"], "speechConfig": {"voiceConfig": {"prebuiltVoiceConfig": {"voiceName": "nova"}}}}});
    let r = gemini_post(&s, "/v1beta/models/mediaco/say:generateContent", &speech).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(record(&s, &r).model_type, Some(ModelType::Tts));
    let body: Value = json(r).await;
    let data = &body["candidates"][0]["content"]["parts"][0]["inlineData"];
    assert_eq!(data["mimeType"], "audio/mpeg");
    assert_eq!(body::bytes_of(&data["data"]).unwrap(), b"ID3-audio");
    let sent = s.mock.received()[1].json();
    assert_eq!((sent["input"].as_str(), sent["voice"].as_str()), (Some("hello"), Some("nova")));
}

#[tokio::test]
async fn gemini_predict_long_running_and_operations() {
    let s = types_server().await;
    s.mock.on("/media/videos/up_v2/content", [Step::binary("video/mp4", &b"MP4"[..])]);
    s.mock
        .on("/media/videos/up_v2", [Step::json(200, json!({"id": "up_v2", "object": "video", "status": "completed"}))]);
    s.mock.on("/media/videos", [Step::accepted(json!({"id": "up_v2", "object": "video", "status": "queued"}))]);
    let r =
        gemini_post(&s, "/v1beta/models/mediaco/vid:predictLongRunning", &json!({"instances": [{"prompt": "a cat"}]}))
            .send()
            .await
            .unwrap();
    assert_eq!(r.status(), 200);
    let op: Value = json(r).await;
    let name = op["name"].as_str().unwrap().to_owned();
    assert!(name.starts_with("operations/vj_"), "{op}");
    assert_eq!(op["done"], false);
    assert_eq!(s.mock.received()[0].json()["prompt"], "a cat");

    let r = reqwest::Client::new()
        .get(format!("{}/v1beta/{name}", s.base))
        .header("x-goog-api-key", &s.key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let op: Value = json(r).await;
    assert_eq!(op["done"], true, "{op}");
    let uri =
        op["response"]["generateVideoResponse"]["generatedSamples"][0]["video"]["uri"].as_str().unwrap().to_owned();
    assert!(uri.starts_with(&s.base) && uri.ends_with(":download"), "{uri}");
    let r = reqwest::Client::new().get(uri).header("x-goog-api-key", &s.key).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(&r.bytes().await.unwrap()[..], b"MP4");
}
