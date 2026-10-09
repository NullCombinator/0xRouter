//! A server with no third-party adapter and no review model (spec 004, SC-011, US2-7): with
//! `[adapters] builder` pointing at a missing path, `serve`'s startup steps succeed, no adapter
//! alert is raised, slice 003's whole and streamed chat requests are served, and the process
//! spawns no child.

mod common;

use std::collections::BTreeSet;

use common::{SECRET, chat_stream, chat_whole, server_custom};
use nullrouter_adapters::alerts::AlertLog;
use nullrouter_adapters::store::Store;
use serde_json::{Value, json};

/// The pids of the children this process has forked and not yet reaped. Linux exposes them per
/// thread, in `/proc/self/task/<tid>/children` (kernels built with `CONFIG_PROC_CHILDREN`), so
/// every thread's file is read. A thread that exits between the directory listing and the read
/// has no file left and contributes nothing.
fn children() -> BTreeSet<String> {
    let mut pids = BTreeSet::new();
    for task in std::fs::read_dir("/proc/self/task").unwrap() {
        let path = task.unwrap().path().join("children");
        match std::fs::read_to_string(&path) {
            Ok(text) => pids.extend(text.split_whitespace().map(str::to_owned)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("{}: {e}", path.display()),
        }
    }
    pids
}

#[tokio::test]
async fn serves_with_no_adapters_and_no_builder() {
    let s = server_custom(
        |mock| {
            vec![(
                "mockco",
                format!(
                    "schema = 2\nid = \"mockco\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n",
                    mock.url("/v1/chat/completions")
                ),
            )]
        },
        &format!("schema = 1\n[[account]]\nprovider = \"mockco\"\nname = \"main\"\nsecret = \"{SECRET}\"\n"),
        "[adapters]\nbuilder = \"/nonexistent/nullrouter-builder\"\n",
    )
    .await;

    // `serve` opens the adapter store right after the engine, and it must succeed with no
    // adapter installed.
    s.engine.open_adapters().unwrap();

    // No adapter alert was raised: `serve` has nothing to warn about. The alert log is the only
    // place an adapter warning is kept, and `raise` writes one for each warning it logs.
    let store = Store::open(s.home()).unwrap();
    assert!(AlertLog::open(&store).list().unwrap().is_empty(), "an adapter alert was raised");

    let before = children();

    let client = reqwest::Client::new();
    let url = format!("{}/v1/chat/completions", s.base);
    let body = |stream: bool| {
        json!({"model": "mockco/m1", "stream": stream, "messages": [{"role": "user", "content": "hi"}]}).to_string()
    };

    s.mock.push([chat_whole()]);
    let r = client
        .post(&url)
        .bearer_auth(&s.key)
        .header("content-type", "application/json")
        .body(body(false))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let whole: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
    assert_eq!(whole["choices"][0]["message"]["content"], "hi there", "{whole}");

    s.mock.push([chat_stream()]);
    let r = client
        .post(&url)
        .bearer_auth(&s.key)
        .header("content-type", "application/json")
        .body(body(true))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let text = r.text().await.unwrap();
    let deltas: String = text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter(|d| *d != "[DONE]")
        .filter_map(|d| serde_json::from_str::<Value>(d).ok())
        .filter_map(|v| v["choices"][0]["delta"]["content"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(deltas, "Hello", "{text}");

    // Children forked during the requests: none. The check sees children that are still
    // unreaped, so a child that was forked and already reaped would not be listed.
    let spawned: Vec<String> = children().difference(&before).cloned().collect();
    assert!(spawned.is_empty(), "the server spawned child processes: {spawned:?}");
}
