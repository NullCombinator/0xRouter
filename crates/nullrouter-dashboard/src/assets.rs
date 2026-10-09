//! The style sheets and the font, embedded and served under one content hash
//! (`/assets/<hash>/<file>`, research R11): cacheable for a day, never fetched from outside the
//! machine, and open without the cookie because they carry no 0router data. Icons are drawn
//! inline ([`crate::components::icon`]).

use std::sync::OnceLock;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use crate::headers::IMMUTABLE;

struct Asset {
    name: &'static str,
    mime: &'static str,
    bytes: &'static [u8],
}

const ASSETS: &[Asset] = &[
    Asset { name: "fonts.css", mime: "text/css; charset=utf-8", bytes: include_bytes!("../style/fonts.css") },
    Asset { name: "tokens.css", mime: "text/css; charset=utf-8", bytes: include_bytes!("../style/tokens.css") },
    Asset { name: "dashboard.css", mime: "text/css; charset=utf-8", bytes: include_bytes!("../style/dashboard.css") },
    Asset { name: "inter-latin.woff2", mime: "font/woff2", bytes: include_bytes!("../assets/inter-latin.woff2") },
];

/// The sheets every page links, in cascade order.
pub const STYLESHEETS: [&str; 3] = ["fonts.css", "tokens.css", "dashboard.css"];

/// One hash over every asset, so `fonts.css` can name the font by a relative `url()`.
fn hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        let mut h = Sha256::new();
        for a in ASSETS {
            h.update(a.name.as_bytes());
            h.update(a.bytes);
        }
        h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
    })
}

/// The address of the asset `name`.
pub fn href(name: &str) -> String {
    format!("/assets/{}/{name}", hash())
}

/// `GET /assets/<hash>/<file>`: the asset when the hash is the current one; 404 otherwise.
pub fn serve(path: &str) -> Response {
    let rest = path.strip_prefix("/assets/").unwrap_or_default();
    let found =
        rest.split_once('/').filter(|(h, _)| *h == hash()).and_then(|(_, name)| ASSETS.iter().find(|a| a.name == name));
    match found {
        Some(a) => (StatusCode::OK, [(header::CONTENT_TYPE, a.mime), (header::CACHE_CONTROL, IMMUTABLE)], a.bytes)
            .into_response(),
        None => (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "No such asset.\n")
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_asset_is_served_under_the_current_hash_only() {
        let ok = serve(&href("dashboard.css"));
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(ok.headers()[header::CACHE_CONTROL], IMMUTABLE);
        assert_eq!(ok.headers()[header::CONTENT_TYPE], "text/css; charset=utf-8");
        assert_eq!(serve("/assets/0000/dashboard.css").status(), StatusCode::NOT_FOUND);
        assert_eq!(serve(&href("nope.css")).status(), StatusCode::NOT_FOUND);
        assert_eq!(serve(&href("inter-latin.woff2")).headers()[header::CONTENT_TYPE], "font/woff2");
    }

    #[test]
    fn the_font_sheet_names_the_font_relatively() {
        let css = std::str::from_utf8(ASSETS[0].bytes).unwrap();
        assert!(css.contains("url(\"inter-latin.woff2\")"), "{css}");
        assert!(!css.contains("http"), "nothing is fetched from outside the machine");
    }
}
