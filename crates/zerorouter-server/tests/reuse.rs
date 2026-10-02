//! Upstream connection reuse (T139, SC-011, FR-021): sequential requests to one provider
//! within its keep-alive window, whole and streamed, all ride the first connection.

mod common;

use common::{chat_stream, chat_whole, server};
use serde_json::json;

const N: usize = 8;

#[tokio::test]
async fn sequential_requests_to_one_provider_share_one_connection() {
    let s = server().await;
    s.mock.respond(|r| if r.json()["stream"] == true { chat_stream() } else { chat_whole() });
    let client = reqwest::Client::new();
    for i in 0..N {
        let stream = i % 2 == 1;
        let body = json!({"model": "mockco/m1", "stream": stream, "messages": [{"role": "user", "content": "hi"}]});
        let r = client
            .post(format!("{}/v1/chat/completions", s.base))
            .bearer_auth(&s.key)
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "request {i}");
        // Read to the end, so the upstream body is done before the next request.
        r.bytes().await.unwrap();
    }
    assert_eq!(s.mock.received().len(), N);
    assert_eq!(s.mock.connections(), 1, "every request after the first reuses the upstream connection");
}
