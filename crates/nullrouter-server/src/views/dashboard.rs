//! `dashboard status`: whether the dashboard is on, where, whether it is serving and why not,
//! and when its token was issued. The token itself is never here: `dashboard.toml` holds only
//! its digest and this view reads only the time.
//!
//! `--json` (spec 009 data-model.md): `{"enabled","listen","server","serving","error","token_issued"}`.
//! With a server running, `enabled`, `listen`, `serving` and `error` are what it is doing (the
//! config may have changed since it started); with none, `serving` is `null` and the first two
//! come from `config.toml`.

use nullrouter_engine::files::DashboardToken;
use nullrouter_registry::{OperatorHome, load_config};
use serde_json::{Value, json};

use super::{Live, View, ViewError};

pub const NEEDS: &[&str] = &["server.status"];

pub fn build(home: &OperatorHome, _args: &Value, live: &Live) -> Result<View, ViewError> {
    let token = DashboardToken::load(home.path()).map_err(ViewError::failed)?;
    let json = match live.answer("server.status").filter(|a| a["ok"] == true) {
        Some(a) => {
            let d = &a["dashboard"];
            json!({
                "enabled": d["enabled"],
                "listen": d["listen"],
                "server": "running",
                "serving": d["serving"],
                "error": d["error"],
                "token_issued": token.issued,
            })
        }
        None => {
            let config = load_config(home).map_err(|e| ViewError::failed(format!("startup failed:\n{e}")))?;
            json!({
                "enabled": config.dashboard.enabled,
                "listen": config.dashboard.listen,
                "server": "none",
                "serving": null,
                "error": null,
                "token_issued": token.issued,
            })
        }
    };
    Ok(View::new(json))
}

#[cfg(test)]
mod tests {
    use nullrouter_engine::testkit::homes;

    use super::*;

    fn running(dashboard: Value) -> Live {
        let mut live = Live { running: true, ..Live::none() };
        live.answers.insert("server.status", Some(json!({"ok": true, "client_listen": null, "dashboard": dashboard})));
        live
    }

    fn view(home: &OperatorHome, live: &Live) -> Value {
        build(home, &json!({}), live).unwrap().json
    }

    #[test]
    fn with_no_server_it_reads_the_file_and_serving_is_null() {
        let dir = homes::empty();
        let home = OperatorHome::new(dir.path());
        assert_eq!(
            view(&home, &Live::none()),
            json!({"enabled": true, "listen": "127.0.0.1:20130", "server": "none", "serving": null,
                   "error": null, "token_issued": null})
        );
        std::fs::write(
            dir.path().join("config.toml"),
            "schema = 1\n\n[dashboard]\nenabled = false\nlisten = \"127.0.0.1:9\"\n",
        )
        .unwrap();
        let v = view(&home, &Live::none());
        assert_eq!((&v["enabled"], &v["listen"]), (&json!(false), &json!("127.0.0.1:9")));
    }

    #[test]
    fn a_running_server_says_what_it_is_doing() {
        let dir = homes::empty();
        let home = OperatorHome::new(dir.path());
        let v = view(
            &home,
            &running(json!({"enabled": true, "listen": "127.0.0.1:20130", "serving": true, "error": null})),
        );
        assert_eq!((v["server"].as_str(), &v["serving"], &v["error"]), (Some("running"), &json!(true), &Value::Null));

        let failed = json!({"enabled": true, "listen": "127.0.0.1:20130", "serving": false,
                            "error": "127.0.0.1:20130: Address already in use (os error 98)"});
        let v = view(&home, &running(failed));
        assert_eq!(v["serving"], false);
        assert_eq!(v["error"], "127.0.0.1:20130: Address already in use (os error 98)");

        let off = json!({"enabled": false, "listen": "127.0.0.1:20130", "serving": false, "error": null});
        let v = view(&home, &running(off));
        assert_eq!((&v["enabled"], &v["serving"]), (&json!(false), &json!(false)));
    }

    #[test]
    fn a_refusing_server_is_read_as_no_server() {
        let dir = homes::empty();
        let mut live = Live { running: true, ..Live::none() };
        live.answers.insert("server.status", Some(json!({"ok": false, "error": "operator socket: refused"})));
        assert_eq!(view(&OperatorHome::new(dir.path()), &live)["server"], "none");
    }

    #[test]
    fn it_shows_when_the_token_was_issued_and_never_the_token_or_its_digest() {
        let dir = homes::empty();
        let digest = DashboardToken::digest_of("nrd_secret");
        DashboardToken { digest: Some(digest.clone()), issued: Some("2026-10-06T08:00:00Z".into()) }
            .save(dir.path())
            .unwrap();
        let v = view(&OperatorHome::new(dir.path()), &Live::none());
        assert_eq!(v["token_issued"], "2026-10-06T08:00:00Z");
        let text = v.to_string();
        assert!(!text.contains("nrd_secret") && !text.contains(&digest), "{text}");
    }
}
