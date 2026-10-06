//! `GET /signin` and `POST /signin` (contracts/dashboard-http.md "Access"): before a token
//! exists the page names the command that issues one; after, it is the token form. The frame and
//! the style arrive with the rest of the pages; this page is plain markup.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::header::{LOCATION, ORIGIN, SET_COOKIE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use maud::{DOCTYPE, Markup, html};

use crate::Shared;
use crate::access::{self, Gate};

/// What the sign-in page shows.
pub enum Screen {
    /// No token has been issued.
    NoToken,
    /// The form; `wrong` after a token that wasn't the current one.
    Form { next: String, wrong: bool },
}

pub fn page(screen: &Screen) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                title { "Sign in · 0Router Proxy" }
            }
            body {
                main {
                    h1 { "0Router Proxy" }
                    @match screen {
                        Screen::NoToken => {
                            p { "No dashboard token yet. Run " code { "nullrouter dashboard token" } "." }
                        }
                        Screen::Form { next, wrong } => {
                            @if *wrong {
                                p role="alert" { "That token is not the current one." }
                            }
                            form method="post" action="/signin" {
                                label for="token" { "Dashboard token" }
                                input id="token" type="password" name="token" autocomplete="off" required;
                                input type="hidden" name="next" value=(next);
                                button type="submit" { "Sign in" }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn html_response(status: StatusCode, screen: &Screen) -> Response {
    (status, Html(page(screen).into_string())).into_response()
}

/// A `303 See Other` to `to`, which is a sanitised path; one a header can't carry goes to `/`.
pub fn see_other(to: &str) -> Response {
    let location = HeaderValue::try_from(to).unwrap_or_else(|_| HeaderValue::from_static("/"));
    (StatusCode::SEE_OTHER, [(LOCATION, location)]).into_response()
}

pub async fn get(State(shared): State<Arc<Shared>>, req: Request) -> Response {
    match access::gate(&shared.engine, req.headers()) {
        Gate::NoToken => html_response(StatusCode::OK, &Screen::NoToken),
        Gate::SignedIn => see_other("/"),
        Gate::SignedOut => {
            let query = access::pairs(req.uri().query().unwrap_or_default());
            let next = access::sanitize_next(access::field(&query, "next").unwrap_or("/"));
            html_response(StatusCode::OK, &Screen::Form { next, wrong: false })
        }
    }
}

pub async fn post(State(shared): State<Arc<Shared>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    if !access::origin_ok(&shared.rule, parts.headers.get(ORIGIN)) {
        return (StatusCode::FORBIDDEN, "This sign-in did not come from the dashboard.\n").into_response();
    }
    match access::gate(&shared.engine, &parts.headers) {
        Gate::NoToken => return html_response(StatusCode::OK, &Screen::NoToken),
        Gate::SignedIn => return see_other("/"),
        Gate::SignedOut => {}
    }
    // A token is 47 characters; nothing here needs more than a few KiB.
    let body = to_bytes(body, 8 * 1024).await.unwrap_or_default();
    let form = access::pairs(&String::from_utf8_lossy(&body));
    let query = access::pairs(parts.uri.query().unwrap_or_default());
    let next = access::sanitize_next(access::field(&form, "next").or(access::field(&query, "next")).unwrap_or("/"));
    let presented = access::field(&form, "token").unwrap_or_default();
    let digest = shared.engine.snapshot().dashboard.digest.clone().unwrap_or_default();
    if access::token_matches(&digest, presented) {
        shared.access.right_token();
        let mut response = see_other(&next);
        if let Ok(cookie) = HeaderValue::try_from(access::set_cookie(presented)) {
            response.headers_mut().insert(SET_COOKIE, cookie);
        }
        return response;
    }
    tokio::time::sleep(shared.access.wrong_token()).await;
    html_response(StatusCode::UNAUTHORIZED, &Screen::Form { next, wrong: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_no_token_page_names_the_command_and_has_no_form() {
        let text = page(&Screen::NoToken).into_string();
        assert!(text.contains("nullrouter dashboard token"), "{text}");
        assert!(!text.contains("<form") && !text.contains("name=\"token\""), "{text}");
        assert!(text.contains("lang=\"en\""));
    }

    #[test]
    fn the_form_carries_next_escaped_and_the_wrong_token_message_only_after_a_miss() {
        let first = page(&Screen::Form { next: "/usage?before=rq_1&x=\"y\"".into(), wrong: false }).into_string();
        assert!(first.contains("name=\"token\"") && first.contains("method=\"post\""), "{first}");
        assert!(first.contains("&amp;") && first.contains("&quot;"), "next is escaped: {first}");
        assert!(!first.contains("That token is not the current one."));
        let again = page(&Screen::Form { next: "/".into(), wrong: true }).into_string();
        assert!(again.contains("That token is not the current one."), "{again}");
    }

    #[test]
    fn a_redirect_a_header_cannot_carry_goes_to_the_root() {
        let r = see_other("/ok");
        assert_eq!((r.status(), r.headers()[LOCATION].to_str().unwrap()), (StatusCode::SEE_OTHER, "/ok"));
        let r = see_other("/bad\nvalue");
        assert_eq!(r.headers()[LOCATION].to_str().unwrap(), "/");
    }
}
