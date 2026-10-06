//! A reload carries new priorities, overrides and amortization lengths into the next decision
//! while the router keeps its warm store and ledgers (spec 006, T057): an account whose priority
//! changed, or that was added or enabled again, starts its deficit at 0, and a changed
//! amortization length keeps what is owed until the new window's next boundary.

mod common;

use std::time::SystemTime;

use common::*;
use nullrouter_engine::accounts;
use nullrouter_engine::route;
use nullrouter_engine::routing::view::TargetView;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const SUB: &str = r#"
[routing.cache]
mode = "automatic"
lifetime = "5m"
min_tokens = 0

[[routing.window]]
name = "5h"
length = "5h"
unit = "weighted_tokens"
capacity = 1000000
"#;

fn fleet() -> Fleet {
    Fleet::new()
        .provider("alpha", SUB, &[("5h", "tokens")])
        .account("alpha", "a", 1.0)
        .account("alpha", "b", 1.0)
        .account("alpha", "c", 1.0)
        .unified(&[("alpha", "m1")])
}

async fn cold(f: &FleetSetup, n: usize) {
    for i in 0..n {
        let body = json!({"model": "u", "stream": false, "messages": [
            {"role": "system", "content": "You are a careful assistant."},
            {"role": "user", "content": format!("question {i} {}", "word ".repeat(200))},
        ]});
        let req = request(&f.setup, "openai-chat", "u", body, &format!("ak_{i}"), CancellationToken::new());
        let id = req.id.clone();
        f.setup.engine.text(f.setup.engine.snapshot(), req).await.expect("served");
        settled(&f.setup, &id).await;
    }
}

fn view(f: &FleetSetup) -> TargetView {
    let st = f.setup.engine.snapshot();
    route::view_all(&f.setup.engine, &st, Some("u"), SystemTime::now()).remove(0)
}

fn deficit(v: &TargetView, name: &str) -> i64 {
    v.accounts.iter().find(|a| a.account == name).unwrap().deficit
}

fn rewrite(f: &FleetSetup, edit: impl FnOnce(String) -> String) {
    let path = f.setup._dir.path().join(accounts::FILE);
    let text = edit(std::fs::read_to_string(&path).unwrap());
    nullrouter_engine::files::write_private(&path, &text).unwrap();
}

#[tokio::test]
async fn a_new_priority_or_a_re_enabled_account_starts_again_at_zero_and_the_rest_keep_their_deficits() {
    let f = fleet().build().await;
    cold(&f, 5).await;
    let before = view(&f);
    assert!(
        before.accounts.iter().all(|a| a.deficit != 0),
        "{:?}",
        before.accounts.iter().map(|a| a.deficit).collect::<Vec<_>>()
    );

    // Priority 2 on `a`: its deficit starts over, the others' stay, and its share doubles in the view.
    rewrite(&f, |t| t.replacen("name = \"a\"\n", "name = \"a\"\npriority = 2.0\n", 1));
    f.setup.engine.reload().await.unwrap();
    let after = view(&f);
    assert_eq!(after.accounts.iter().find(|a| a.account == "a").unwrap().priority, 2.0, "reload saw the file");
    assert_eq!(deficit(&after, "a"), 0);
    assert_eq!((deficit(&after, "b"), deficit(&after, "c")), (deficit(&before, "b"), deficit(&before, "c")));
    let share = |v: &TargetView, n: &str| v.accounts.iter().find(|a| a.account == n).unwrap().share.unwrap();
    assert!(
        (share(&after, "a") / share(&after, "b") - 2.0).abs() < 1e-6,
        "{} / {}",
        share(&after, "a"),
        share(&after, "b")
    );

    // `b` disabled and enabled again: it starts at 0 too.
    cold(&f, 4).await;
    let owed = deficit(&view(&f), "b");
    assert_ne!(owed, 0);
    rewrite(&f, |t| t.replacen("name = \"b\"\n", "name = \"b\"\ndisabled = true\n", 1));
    f.setup.engine.reload().await.unwrap();
    assert!(view(&f).accounts.iter().all(|a| a.account != "b"), "a disabled account is not in the view");
    rewrite(&f, |t| t.replacen("disabled = true\n", "", 1));
    f.setup.engine.reload().await.unwrap();
    assert_eq!(deficit(&view(&f), "b"), 0);
}

#[tokio::test]
async fn a_changed_amortization_length_keeps_what_is_owed_for_now() {
    let f = fleet().build().await;
    cold(&f, 4).await;
    let before = view(&f);
    assert_eq!(before.amortization_window.length.as_secs(), 5 * 3600);
    std::fs::write(
        f.setup._dir.path().join("config.toml"),
        std::fs::read_to_string(f.setup._dir.path().join("config.toml")).unwrap()
            + "\n[routing.amortization_for]\nu = \"1h\"\n",
    )
    .unwrap();
    f.setup.engine.reload().await.unwrap();
    let after = view(&f);
    assert_eq!(after.amortization_window.length.as_secs(), 3600);
    for name in ["a", "b", "c"] {
        assert_eq!(deficit(&after, name), deficit(&before, name), "{name}");
    }
}
