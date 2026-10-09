//! The adapter catalogue client (contracts/catalogue.md).
//!
//! Reached only from operator commands: nothing in `serve` or a reload names this module
//! (FR-031, SC-012). A fetch is HTTPS only, follows at most three redirects and only to the same
//! host, caps the index at 1 MiB and the archive at 2 MiB while streaming, and times out after
//! 30 s. The archive is checked against `sha256`, unpacked in memory by [`crate::unpack`] (the
//! same rules as a local install), checked against `source_fp`, written under
//! `adapters/.staging/`, and handed to [`crate::install`] with `origin = {catalogue = url}`.

use std::collections::BTreeSet;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::fingerprint::{self, SourceFp};
use crate::store::{Origin, Store};
use crate::unpack;
use crate::{HarnessName, InstallError, InstallOptions, Installed};

pub const MAX_INDEX_BYTES: usize = 1024 * 1024;
pub const MAX_ARCHIVE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_REDIRECTS: usize = 3;
pub const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum CatalogueError {
    #[error("{0}: only https URLs are fetched")]
    NotHttps(String),
    #[error("fetching {url}: {reason}")]
    Fetch { url: String, reason: String },
    #[error("{url}: more than {limit} bytes")]
    TooLarge { url: String, limit: usize },
    #[error("the catalogue index is not valid: {0}")]
    BadIndex(String),
    #[error("the catalogue lists no harness {0:?}")]
    UnknownHarness(String),
    #[error("the catalogue lists no version {1:?} of {0}")]
    UnknownVersion(String, String),
    #[error("the archive's sha256 is {got}, the catalogue says {want}")]
    HashMismatch { want: String, got: String },
    #[error("the unpacked source is {got}, the catalogue says {want}")]
    FpMismatch { want: String, got: String },
    #[error("the archive is refused: {0}")]
    Unpack(#[from] unpack::UnpackError),
    #[error("staging: {0}")]
    Staging(String),
    #[error(transparent)]
    Install(#[from] InstallError),
}

impl CatalogueError {
    /// The stable code operators and tests see.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotHttps(_) => "catalogue_not_https",
            Self::Fetch { .. } => "catalogue_fetch_failed",
            Self::TooLarge { .. } => "catalogue_too_large",
            Self::BadIndex(_) => "catalogue_bad_index",
            Self::UnknownHarness(_) => "catalogue_unknown_harness",
            Self::UnknownVersion(..) => "catalogue_unknown_version",
            Self::HashMismatch { .. } => "catalogue_hash_mismatch",
            Self::FpMismatch { .. } => "catalogue_fp_mismatch",
            Self::Unpack(_) => "catalogue_unpack_refused",
            Self::Staging(_) => "catalogue_staging_failed",
            Self::Install(_) => "catalogue_install_failed",
        }
    }
}

/// `catalogue/index.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub schema: u32,
    #[serde(default)]
    pub entry: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub harness: String,
    pub summary: String,
    #[serde(default)]
    pub homepage: Option<String>,
    pub style: String,
    #[serde(default)]
    pub version: Vec<Version>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Version {
    pub semver: String,
    pub source: String,
    pub sha256: String,
    pub source_fp: String,
    pub kit: String,
}

impl Index {
    /// Parses and checks the rules the contract lists (CI checks the same in the repository).
    pub fn parse(text: &str) -> Result<Self, CatalogueError> {
        let bad = CatalogueError::BadIndex;
        let index: Index = toml::from_str(text).map_err(|e| bad(e.to_string()))?;
        if index.schema != 1 {
            return Err(bad(format!("schema {} is not supported", index.schema)));
        }
        let mut harnesses = BTreeSet::new();
        for entry in &index.entry {
            let name = HarnessName::new(&entry.harness).map_err(|e| bad(e.to_string()))?;
            if name.is_builtin() {
                return Err(bad(format!("{name} is built in")));
            }
            if !harnesses.insert(&entry.harness) {
                return Err(bad(format!("{name} is listed twice")));
            }
            let mut versions = BTreeSet::new();
            for v in &entry.version {
                let semver = semver::Version::parse(&v.semver).map_err(|e| bad(format!("{name} {}: {e}", v.semver)))?;
                if !versions.insert(semver) {
                    return Err(bad(format!("{name} {} is listed twice", v.semver)));
                }
                if !v.source.starts_with("https://") {
                    return Err(bad(format!("{name} {}: source is not https", v.semver)));
                }
                let hex = v.sha256.len() == 64 && v.sha256.bytes().all(|b| b.is_ascii_hexdigit());
                if !hex {
                    return Err(bad(format!("{name} {}: sha256 is not 64 hex digits", v.semver)));
                }
                SourceFp::parse(&v.source_fp).map_err(|e| bad(e.to_string()))?;
            }
        }
        Ok(index)
    }

    pub fn entry(&self, harness: &str) -> Option<&Entry> {
        self.entry.iter().find(|e| e.harness == harness)
    }
}

impl Entry {
    /// The version named `semver`, or the highest one.
    pub fn pick(&self, semver: Option<&str>) -> Option<&Version> {
        match semver {
            Some(want) => self.version.iter().find(|v| v.semver == want),
            None => self.version.iter().max_by_key(|v| semver::Version::parse(&v.semver).ok()),
        }
    }
}

/// The HTTP client of the catalogue commands.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
}

impl Client {
    pub fn new() -> Result<Self, CatalogueError> {
        Self::build(None)
    }

    /// A client that trusts only the DER certificate `root`. For tests against a local server
    /// with its own CA; production uses [`Client::new`], which trusts the default roots.
    pub fn with_extra_root(root: &[u8]) -> Result<Self, CatalogueError> {
        Self::build(Some(root))
    }

    fn build(root: Option<&[u8]>) -> Result<Self, CatalogueError> {
        let fail = |e: reqwest::Error| CatalogueError::Fetch { url: String::new(), reason: chain(&e) };
        let mut b = reqwest::Client::builder().https_only(true).redirect(redirect_policy()).timeout(TIMEOUT);
        if let Some(der) = root {
            b = b.tls_certs_only([reqwest::Certificate::from_der(der).map_err(fail)?]).no_proxy();
        }
        Ok(Self { http: b.build().map_err(fail)? })
    }

    /// Fetches and checks the index.
    pub async fn fetch_index(&self, url: &str) -> Result<Index, CatalogueError> {
        let bytes = self.get(url, MAX_INDEX_BYTES).await?;
        let text = std::str::from_utf8(&bytes).map_err(|_| CatalogueError::BadIndex("not UTF-8".to_owned()))?;
        Index::parse(text)
    }

    /// Installs `harness` (`semver`: the highest if `None`) from the catalogue at `index_url`.
    /// `opts.origin` is replaced with the archive's URL.
    pub async fn install(
        &self,
        home: &Path,
        index_url: &str,
        harness: &str,
        semver: Option<&str>,
        mut opts: InstallOptions<'_>,
    ) -> Result<Installed, CatalogueError> {
        let index = self.fetch_index(index_url).await?;
        let entry = index.entry(harness).ok_or_else(|| CatalogueError::UnknownHarness(harness.to_owned()))?;
        let version = entry
            .pick(semver)
            .ok_or_else(|| CatalogueError::UnknownVersion(harness.to_owned(), semver.unwrap_or("latest").to_owned()))?;

        let archive = self.get(&version.source, MAX_ARCHIVE_BYTES).await?;
        let got = format!("{:x}", Sha256::digest(&archive));
        if !got.eq_ignore_ascii_case(&version.sha256) {
            return Err(CatalogueError::HashMismatch { want: version.sha256.clone(), got });
        }
        let files = unpack::read_archive(&archive[..])?;
        let got = fingerprint::of_files(&files);
        if got.as_str() != version.source_fp {
            return Err(CatalogueError::FpMismatch { want: version.source_fp.clone(), got: got.to_string() });
        }

        let staging_err = |e: &dyn std::fmt::Display| CatalogueError::Staging(e.to_string());
        let store = Store::open(home).map_err(InstallError::from)?;
        let staging = store.root().join(".staging");
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&staging).map_err(|e| staging_err(&e))?;
        let dir = tempfile::Builder::new().prefix("catalogue-").tempdir_in(&staging).map_err(|e| staging_err(&e))?;
        unpack::write_tree(&files, dir.path())?;
        opts.origin = Origin::Catalogue(version.source.clone());
        Ok(crate::install(home, dir.path(), &opts).await?)
    }

    /// A GET with the https check and a running size cap.
    async fn get(&self, url: &str, limit: usize) -> Result<Vec<u8>, CatalogueError> {
        if !url.starts_with("https://") {
            return Err(CatalogueError::NotHttps(url.to_owned()));
        }
        let fetch = |reason: String| CatalogueError::Fetch { url: url.to_owned(), reason };
        let too_large = || CatalogueError::TooLarge { url: url.to_owned(), limit };
        let mut response = self.http.get(url).send().await.map_err(|e| fetch(chain(&e)))?;
        if !response.status().is_success() {
            return Err(fetch(format!("status {}", response.status().as_u16())));
        }
        if response.content_length().is_some_and(|n| n > limit as u64) {
            return Err(too_large());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| fetch(chain(&e)))? {
            if body.len() + chunk.len() > limit {
                return Err(too_large());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

/// Same host as the first request, HTTPS, at most [`MAX_REDIRECTS`] hops.
fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            return attempt.error("more than 3 redirects");
        }
        if attempt.url().scheme() != "https" {
            return attempt.error("redirect to a non-https URL");
        }
        let same = attempt.previous().first().is_some_and(|f| f.host_str() == attempt.url().host_str());
        if same { attempt.follow() } else { attempt.error("redirect to another host") }
    })
}

/// An error and its sources on one line.
fn chain(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut next = e.source();
    while let Some(s) = next {
        text.push_str(": ");
        text.push_str(&s.to_string());
        next = s.source();
    }
    text
}
