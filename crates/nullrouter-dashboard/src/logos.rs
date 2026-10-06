//! `GET /logos/<hash>/<id>.png` (research R10, FR-008): a provider's checked logo from the
//! registry snapshot, behind the cookie like a page.
//!
//! The registry checked each logo at load (PNG signature, `IHDR` within 256 px, at most 64 KiB) and
//! keeps the bytes of the ones that passed, with a hash of them. The hash is part of the address,
//! so a changed logo gets a new one and the old address stops answering; that is what lets a
//! logo be cached like an asset. The type is fixed (`image/png`) and `nosniff` comes with every
//! response (`headers::enforce`), so a browser treats the bytes only as an image (FR-042).

use std::collections::BTreeMap;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use nullrouter_engine::state::Engine;
use nullrouter_registry::Registry;

use crate::headers::IMMUTABLE;

/// Each provider's logo address in the current snapshot. A provider without one isn't listed;
/// its card shows the text icon.
#[derive(Debug, Clone, Default)]
pub struct Index(BTreeMap<String, String>);

impl Index {
    /// The addresses for `engine`'s current snapshot.
    pub fn of(engine: &Engine) -> Self {
        Self::of_registry(&engine.snapshot().registry)
    }

    /// The addresses for `registry`'s logos.
    pub fn of_registry(registry: &Registry) -> Self {
        Self(registry.logos().map(|(id, logo)| (id.to_owned(), href(id, logo.hash()))).collect())
    }

    /// `/logos/<hash>/<id>.png`, or `None`.
    pub fn href(&self, provider: &str) -> Option<&str> {
        self.0.get(provider).map(String::as_str)
    }
}

fn href(provider: &str, hash: &str) -> String {
    format!("/logos/{hash}/{provider}.png")
}

/// The logo at `path`, or 404: the provider must have a logo in the current snapshot and `path`
/// must carry that logo's hash.
pub fn serve(engine: &Engine, path: &str) -> Response {
    serve_from(&engine.snapshot().registry, path)
}

/// [`serve`] from `registry`.
pub fn serve_from(registry: &Registry, path: &str) -> Response {
    let found = path
        .strip_prefix("/logos/")
        .and_then(|rest| rest.split_once('/'))
        .and_then(|(hash, file)| Some((hash, file.strip_suffix(".png")?)))
        .and_then(|(hash, id)| registry.logo(id).filter(|logo| logo.hash() == hash));
    match found {
        Some(logo) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, IMMUTABLE)],
            logo.bytes().to_vec(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "No such logo.\n")
            .into_response(),
    }
}
