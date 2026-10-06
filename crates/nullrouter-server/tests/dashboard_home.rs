//! The `dashboard()` fixture home (spec 009, T003) reaches every `check` notice kind a stopped
//! home can show (but a withheld credential, see `homes::dashboard`), so the dashboard suite's notice tests have something on each page.

use nullrouter_engine::testkit::homes;
use nullrouter_registry::OperatorHome;
use nullrouter_server::views::{self, Live};
use serde_json::json;

#[test]
fn dashboard_home_has_every_stopped_notice_kind() {
    let dir = homes::dashboard();
    let home = OperatorHome::new(dir.path());
    let view = views::check::build(&home, &json!({}), &Live::none()).unwrap();
    let (r, x) = (&view.json, &view.extra);
    let some = |v: &serde_json::Value| v.as_array().is_some_and(|a| !a.is_empty());
    assert!(some(&r["pending_conflicts"]), "pending conflict");
    assert!(some(&r["declined"]), "declined");
    assert!(some(&r["skipped"]), "skipped plugin");
    assert!(some(&r["dropped_unified_models"]), "dropped unified model");
    assert!(some(&x["notes"]), "limits note");
    assert!(some(&r["unmetered_windows"]), "unmetered window");
    assert!(some(&r["routing_warnings"]), "routing warning");
    assert!(some(&r["signin"]["tokens_without_account"]), "token without account");
    assert!(some(&r["signin"]["accounts_without_tokens"]), "account without tokens");
    assert!(some(&r["signin"]["file_modes"]), "bad file mode");
    assert!(r["signin"]["errors"].as_array().unwrap().is_empty(), "serve must start on this home");
}
