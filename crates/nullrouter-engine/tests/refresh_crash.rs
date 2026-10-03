//! Refresh across a crash and a restart (spec 005 T048, FR-014, research R9): the rotated
//! refresh token is written before the in-memory swap, so a crash between the two still
//! leaves the newest one in `tokens.toml`; after a restart an expired access token is
//! refreshed before the account serves.

mod common;
mod signin_kit;

use std::sync::Arc;
use std::time::Duration;

use common::{Setup, send, settled};
use nullrouter_engine::signin::SignInHttp;
use nullrouter_engine::signin::refresh::{Persisted, persist, refresh};
use nullrouter_engine::state::Engine;
use nullrouter_registry::OperatorHome;
use signin_kit::{Shape, bearer, kit};

#[tokio::test]
async fn a_crash_after_the_write_keeps_the_newest_refresh_token_and_a_restart_refreshes_before_serving() {
    let k = kit(&[("p", Shape::Device, "5m")], &["a"], Duration::from_secs(2)).await;
    k.idp.set_rotation(true);
    k.sign_in("p", "a", |_| {});
    let first = k.stored("p", "a").refresh_token.unwrap().with_exposed(str::to_owned);

    // The refresh's two steps, then a crash: written, never swapped.
    let engine = k.engine().clone();
    let view = engine.tokens.get("p", "a").unwrap();
    let st = engine.snapshot();
    let http = SignInHttp::with_client(st.http.clone(), true);
    let next = refresh(&http, st.registry.provider("p").unwrap(), &view.entry).await.unwrap();
    assert!(matches!(persist(k.home(), &view.entry, &next).unwrap(), Persisted::Written));
    let in_memory = engine.tokens.get("p", "a").unwrap();
    assert!(in_memory.entry.refresh_token.as_ref().unwrap().matches(&first), "not swapped: the crash came first");
    drop((st, view, in_memory));
    let signin_kit::Kit { s: Setup { _dir: dir, engine: _, mock }, idp } = k;
    drop(engine);

    let newest = next.refresh_token.as_ref().unwrap().with_exposed(str::to_owned);
    assert_ne!(newest, first, "rotated");
    let on_disk = nullrouter_engine::tokens::TokenStore::load(dir.path()).unwrap();
    assert!(on_disk.get("p", "a").unwrap().refresh_token.as_ref().unwrap().matches(&newest));

    // Restart once the stored access token has expired.
    tokio::time::sleep(Duration::from_millis(2300)).await;
    let (engine, _) = Engine::open_parity(OperatorHome::new(dir.path())).unwrap();
    let s = Setup { _dir: dir, engine: Arc::new(engine), mock };
    let (id, res) = send(&s, "p/m1").await;
    assert!(res.is_ok(), "{:?}", res.err());
    assert_eq!(idp.refresh_calls(), 2, "refreshed before serving, with the newest refresh token");
    assert_eq!(idp.token_requests().last().unwrap().get("refresh_token"), Some(newest.as_str()));
    let got = s.mock.received();
    assert_eq!(got.len(), 1, "no rejected attempt");
    assert!(idp.token_valid(&bearer(&got[0])));
    assert_eq!(settled(&s, &id).await.attempts.len(), 1);
}
