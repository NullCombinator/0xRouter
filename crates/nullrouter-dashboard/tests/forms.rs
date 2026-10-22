//! The write side's foundation (T011, SC-005): the checks every post passes before any change,
//! the redirect after one, and that a reload repeats nothing. The route is the test-only
//! `debug.noop` op.

mod common;

use common::{Dash, TOKEN};
use nullrouter_server::views::change_log;
use reqwest::header::{CONTENT_TYPE, COOKIE, LOCATION, ORIGIN};
use reqwest::{Client, StatusCode, redirect};

struct Browser {
    d: Dash,
    http: Client,
    cookie: String,
}

impl Browser {
    async fn start() -> Self {
        let d = Dash::empty().await;
        let http = Client::builder().redirect(redirect::Policy::none()).no_proxy().build().unwrap();
        let cookie = format!("nrd_session_{}={TOKEN}", d.addr.port());
        Self { d, http, cookie }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.d.addr)
    }

    async fn form_token(&self) -> String {
        self.http
            .get(self.url("/debug/form-token"))
            .header(COOKIE, &self.cookie)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }

    async fn post(&self, body: String, origin: Option<&str>) -> reqwest::Response {
        let mut r = self
            .http
            .post(self.url("/do/debug/noop"))
            .header(COOKIE, &self.cookie)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(o) = origin {
            r = r.header(ORIGIN, o);
        }
        r.body(body).send().await.unwrap()
    }

    fn own_origin(&self) -> String {
        format!("http://{}", self.d.addr)
    }

    fn log_lines(&self) -> usize {
        change_log::read(&self.d.home(), 100).unwrap().len()
    }
}

fn location(r: &reqwest::Response) -> String {
    r.headers()[LOCATION].to_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_without_the_token_or_from_elsewhere_is_403_and_logs_nothing() {
    let b = Browser::start().await;
    let t = b.form_token().await;
    assert_eq!(t.len(), 32);
    let own = b.own_origin();
    let wrong = "0".repeat(32);
    let cases = [
        (String::from("_back=%2Fendpoint"), Some(own.as_str())),
        (format!("_t={wrong}&_back=%2Fendpoint"), Some(own.as_str())),
        (format!("_t={t}&_back=%2Fendpoint"), Some("http://evil.example")),
        (format!("_t={t}&_back=%2Fendpoint"), None),
    ];
    for (body, origin) in cases {
        let r = b.post(body.clone(), origin).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{body} {origin:?}");
        assert!(r.text().await.unwrap().contains("didn't come from the dashboard"));
    }
    assert_eq!(b.log_lines(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_without_a_session_goes_to_sign_in() {
    let b = Browser::start().await;
    let r = b.http.post(b.url("/do/debug/noop")).header(ORIGIN, b.own_origin()).body("_t=x").send().await.unwrap();
    assert_eq!((r.status(), location(&r)), (StatusCode::SEE_OTHER, "/signin".to_owned()));
    assert_eq!(b.log_lines(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_get_to_do_is_405() {
    let b = Browser::start().await;
    let r = b.http.get(b.url("/do/debug/noop")).header(COOKIE, &b.cookie).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(b.log_lines(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_op_is_404_and_logs_nothing() {
    let b = Browser::start().await;
    let t = b.form_token().await;
    let r = b
        .http
        .post(b.url("/do/nope/nothing"))
        .header(COOKIE, &b.cookie)
        .header(ORIGIN, b.own_origin())
        .body(format!("_t={t}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    assert_eq!(b.log_lines(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_good_post_is_303_with_done_and_a_reload_repeats_nothing() {
    let b = Browser::start().await;
    let t = b.form_token().await;
    let r = b.post(format!("_t={t}&_back=%2Fsettings%3Fx%3D1&subject=thing"), Some(&b.own_origin())).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let to = location(&r);
    assert!(to.starts_with("/settings?x=1&done="), "{to}");
    let lines = change_log::read(&b.d.home(), 10).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].op.as_str(), lines[0].subject.as_str()), ("debug.noop", "thing"));
    assert!(lines[0].at.contains('T') && (lines[0].at.contains('+') || lines[0].at.contains('-')));

    // The 303's target shows the flash once; reloading it shows nothing and changes nothing.
    let first = b.http.get(b.url(&to)).header(COOKIE, &b.cookie).send().await.unwrap().text().await.unwrap();
    assert!(first.contains("Nothing changed."), "{first}");
    let again = b.http.get(b.url(&to)).header(COOKIE, &b.cookie).send().await.unwrap().text().await.unwrap();
    assert!(!again.contains("Nothing changed."));
    assert_eq!(b.log_lines(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_keeps_the_fields_but_not_secrets_and_reopens_the_window() {
    let b = Browser::start().await;
    let t = b.form_token().await;
    let r = b
        .post(
            format!("_t={t}&_back=%2Fproviders&_window=add&subject=s&fail=Bad+name&secret=hunter2"),
            Some(&b.own_origin()),
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let to = location(&r);
    assert!(to.starts_with("/providers?window=add&done="), "{to}");
    assert!(!to.contains("hunter2"));
    let page = b.http.get(b.url(&to)).header(COOKIE, &b.cookie).send().await.unwrap().text().await.unwrap();
    assert!(page.contains("Bad name"));
    assert!(!page.contains("hunter2"));
    assert_eq!(change_log::read(&b.d.home(), 1).unwrap()[0].result, change_log::Outcome::Refused);
}

#[tokio::test(flavor = "multi_thread")]
async fn saved_not_applied_and_once_flashes() {
    let b = Browser::start().await;
    let t = b.form_token().await;
    let r = b.post(format!("_t={t}&saved=disk+full"), Some(&b.own_origin())).await;
    let to = location(&r);
    assert!(to.starts_with("/endpoint?done="), "{to}");
    let page = b.http.get(b.url(&to)).header(COOKIE, &b.cookie).send().await.unwrap().text().await.unwrap();
    assert!(page.contains("saved but not applied: disk full"), "{page}");
    assert_eq!(change_log::read(&b.d.home(), 1).unwrap()[0].result, change_log::Outcome::SavedNotApplied);

    let r = b.post(format!("_t={t}&once=nr_secret_value"), Some(&b.own_origin())).await;
    let page = b.http.get(b.url(&location(&r))).header(COOKIE, &b.cookie).send().await.unwrap().text().await.unwrap();
    assert!(page.contains("nr_secret_value") && page.contains("only time it is shown"));
    let log = std::fs::read_to_string(change_log::path(&b.d.home())).unwrap();
    assert!(!log.contains("nr_secret_value"));
}
