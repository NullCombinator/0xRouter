//! The operator socket's `server.status` op (spec 009, R8): what `serve` bound, and the
//! dashboard listener's state.

use std::sync::Arc;

use nullrouter_engine::state::Engine;
use nullrouter_engine::status::DashboardStatus;
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn server_status_reports_the_listeners() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, _) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    let engine = Arc::new(engine);

    let before = operator::handle(&engine, &json!({"op": "server.status"})).await;
    assert_eq!(
        before,
        json!({"ok": true, "client_listen": null,
               "dashboard": {"enabled": false, "listen": "", "serving": false, "error": null}})
    );

    engine.status.set_client_listen("127.0.0.1:20129");
    engine.status.set_dashboard(DashboardStatus {
        enabled: true,
        listen: "127.0.0.1:20130".into(),
        serving: false,
        error: Some("127.0.0.1:20130: address in use".into()),
    });
    let after = operator::handle(&engine, &json!({"op": "server.status"})).await;
    assert_eq!(
        after,
        json!({"ok": true, "client_listen": "127.0.0.1:20129",
               "dashboard": {"enabled": true, "listen": "127.0.0.1:20130", "serving": false,
                             "error": "127.0.0.1:20130: address in use"}})
    );
}
