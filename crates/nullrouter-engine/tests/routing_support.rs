//! The routing test support (spec 006, T016, T017): a [`Fleet`] opens, the mock upstream
//! answers with cache usage, and the mock quota follows what was served.

mod common;

use common::{Fleet, send};
use nullrouter_engine::testkit::SimWindow;

const ROUTING: &str = r#"
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

#[tokio::test]
async fn a_fleet_serves_with_cache_usage_and_draws_its_quota_down() {
    let f = Fleet::new()
        .provider("alpha", ROUTING, &[("5h", "tokens")])
        .provider("beta", "", &[])
        .account("alpha", "one", 1.0)
        .account("beta", "two", 2.0)
        .unified(&[("alpha", "m1"), ("beta", "m1")])
        .build()
        .await;
    f.quota.set("alpha/one", vec![SimWindow::new("5h", "tokens", 1_000_000.0, "2026-10-05T00:00:00Z")]);

    let (_, answered) = send(&f.setup, "alpha/m1").await;
    if let Err(e) = &answered {
        panic!("not served: {e:?}");
    }
    assert_eq!(f.cache.requests("alpha/one"), 1);
    assert!(f.quota.used("alpha/one", "5h").unwrap() > 0.0, "the window follows what was served");

    let snap = f.setup.engine.snapshot();
    assert_eq!(snap.accounts.iter().map(|a| a.priority).collect::<Vec<_>>(), [1.0, 2.0]);
}

