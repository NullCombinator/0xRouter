//! The catalogue client (T052) against a local HTTPS server whose CA the client trusts in-test.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nullrouter_adapters::catalogue::{CatalogueError, Client, Index};
use nullrouter_adapters::store::{Origin, Store, VersionState};
use nullrouter_adapters::{BUILD_TIMEOUT, InstallOptions, fingerprint, unpack};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

// ---- the server ----

#[derive(Clone)]
enum Reply {
    Ok(Vec<u8>),
    Redirect(String),
}

type Routes = Arc<Mutex<HashMap<String, Reply>>>;

struct Mock {
    port: u16,
    root: Vec<u8>,
    hits: Arc<AtomicUsize>,
    routes: Routes,
}

impl Mock {
    fn url(&self, path: &str) -> String {
        format!("https://127.0.0.1:{}{path}", self.port)
    }

    fn client(&self) -> Client {
        Client::with_extra_root(&self.root).unwrap()
    }

    fn route(&self, path: &str, reply: Reply) {
        self.routes.lock().unwrap().insert(path.to_owned(), reply);
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// The index at `/index.toml` naming `/noop.tar.gz`, which serves `archive`; the index
    /// claims `sha256` and `fp` when given, the true values otherwise.
    fn catalogue(&self, archive: Vec<u8>, sha256: Option<String>, fp: Option<String>) {
        let fp = fp.unwrap_or_else(|| fingerprint::of_files(&noop_files()).to_string());
        let sha256 = sha256.unwrap_or_else(|| sha(&archive));
        self.route("/index.toml", Reply::Ok(index_text(&self.url("/noop.tar.gz"), &sha256, &fp).into_bytes()));
        self.route("/noop.tar.gz", Reply::Ok(archive));
    }
}

async fn serve() -> Mock {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(vec![]).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf =
        CertificateParams::new(vec!["127.0.0.1".to_owned()]).unwrap().signed_by(&leaf_key, &ca, &ca_key).unwrap();

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(leaf.der().to_vec())],
            PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap(),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let routes: Routes = Arc::default();
    let hits = Arc::new(AtomicUsize::new(0));
    let (shared_routes, counted) = (routes.clone(), hits.clone());
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let (acceptor, routes, hits) = (acceptor.clone(), shared_routes.clone(), counted.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else { return };
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                hits.fetch_add(1, Ordering::SeqCst);
                let text = String::from_utf8_lossy(&request).into_owned();
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let reply = routes.lock().unwrap().get(&path).cloned();
                let out = match reply {
                    Some(Reply::Ok(body)) => {
                        let mut out =
                            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                                .into_bytes();
                        out.extend_from_slice(&body);
                        out
                    }
                    Some(Reply::Redirect(to)) => format!(
                        "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                    None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                };
                let _ = tls.write_all(&out).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    Mock { port, root: ca.der().to_vec(), hits, routes }
}

// ---- packages ----

fn noop_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/noop")
}

/// A `.tar.gz` of `files` under `top/`.
fn archive(top: &str, files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for (path, bytes) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        tar.append_data(&mut h, format!("{top}/{path}"), &bytes[..]).unwrap();
    }
    gzip(tar.into_inner().unwrap())
}

fn gzip(tar: Vec<u8>) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

/// One raw tar entry, with a name `tar::Builder` would refuse.
fn raw_entry(tar: &mut tar::Builder<Vec<u8>>, name: &str, kind: tar::EntryType, link: Option<&str>, body: &[u8]) {
    let mut h = tar::Header::new_gnu();
    h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
    if let Some(link) = link {
        h.as_old_mut().linkname[..link.len()].copy_from_slice(link.as_bytes());
    }
    h.set_entry_type(kind);
    h.set_size(body.len() as u64);
    h.set_mode(0o644);
    h.set_cksum();
    tar.append(&h, body).unwrap();
}

fn raw_archive(name: &str, kind: tar::EntryType, link: Option<&str>) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    let body: &[u8] = if kind == tar::EntryType::Regular { b"x" } else { b"" };
    raw_entry(&mut tar, name, kind, link, body);
    gzip(tar.into_inner().unwrap())
}

fn noop_files() -> unpack::Files {
    let mut files = unpack::read_dir_tree(&noop_dir()).unwrap();
    files.sort();
    files
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn index_text(source: &str, sha256: &str, fp: &str) -> String {
    format!(
        "schema = 1\n\n[[entry]]\nharness = \"noop\"\nsummary = \"s\"\nstyle = \"openai-chat\"\n\n\
         [[entry.version]]\nsemver = \"0.1.0\"\nsource = \"{source}\"\nsha256 = \"{sha256}\"\n\
         source_fp = \"{fp}\"\nkit = \"1\"\n"
    )
}

// ---- tests ----

fn opts(styles: &[&'static str]) -> InstallOptions<'_> {
    InstallOptions {
        styles,
        // No builder: the install ends `queued` / `builder_not_installed`, deterministically.
        builder: Some("/nonexistent/nullrouter-builder"),
        origin: Origin::Local(String::new()),
        build_timeout: BUILD_TIMEOUT,
    }
}

async fn install(mock: &Mock, home: &Path) -> Result<nullrouter_adapters::Installed, CatalogueError> {
    mock.client().install(home, &mock.url("/index.toml"), "noop", None, opts(&["openai-chat"])).await
}

fn staged(home: &Path) -> usize {
    std::fs::read_dir(home.join("adapters/.staging")).map(|d| d.count()).unwrap_or(0)
}

#[test]
fn the_index_parses_and_unknown_keys_are_refused() {
    let good = index_text("https://example.com/a.tar.gz", &"a".repeat(64), &format!("sha256:{}", "b".repeat(64)));
    let index = Index::parse(&good).unwrap();
    assert_eq!(index.entry("noop").unwrap().pick(None).unwrap().semver, "0.1.0");
    for (what, text) in [
        ("top-level key", format!("{good}\nextra = 1\n")),
        ("entry key", good.replace("style = ", "colour = \"x\"\nstyle = ")),
        ("version key", good.replace("kit = ", "extra = 1\nkit = ")),
        ("http source", good.replace("https://example.com", "http://example.com")),
        ("short sha256", good.replace(&"a".repeat(64), "abc")),
        ("schema 2", good.replace("schema = 1", "schema = 2")),
        ("built-in harness", good.replace("\"noop\"", "\"hermes\"")),
        ("duplicate harness", format!("{good}\n{}", good.split_once("\n\n").unwrap().1)),
    ] {
        let err = Index::parse(&text).expect_err(what);
        assert_eq!(err.code(), "catalogue_bad_index", "{what}");
    }
}

#[tokio::test]
async fn a_good_catalogue_install_stores_the_version_with_a_catalogue_origin() {
    let mock = serve().await;
    mock.catalogue(archive("noop-0.1.0", &noop_files()), None, None);
    let home = tempfile::tempdir().unwrap();
    let done = install(&mock, home.path()).await.unwrap();
    assert_eq!(done.state, VersionState::Queued);
    assert_eq!(done.reason, "builder_not_installed");
    let store = Store::open(home.path()).unwrap();
    let index = store.load_index().unwrap();
    let entry = index.version(&done.harness, &done.version).unwrap();
    assert_eq!(entry.origin, Origin::Catalogue(mock.url("/noop.tar.gz")));
    assert_eq!(staged(home.path()), 0, "staging is cleaned up");
}

#[tokio::test]
async fn a_hash_mismatch_is_refused_and_nothing_is_unpacked() {
    let mock = serve().await;
    mock.catalogue(archive("noop", &noop_files()), Some("0".repeat(64)), None);
    let home = tempfile::tempdir().unwrap();
    let err = install(&mock, home.path()).await.unwrap_err();
    assert_eq!(err.code(), "catalogue_hash_mismatch");
    assert!(!home.path().join("adapters").exists(), "no store, no staging");
}

#[tokio::test]
async fn a_fingerprint_mismatch_is_refused_before_the_gate() {
    let mock = serve().await;
    mock.catalogue(archive("noop", &noop_files()), None, Some(format!("sha256:{}", "c".repeat(64))));
    let home = tempfile::tempdir().unwrap();
    let err = install(&mock, home.path()).await.unwrap_err();
    assert_eq!(err.code(), "catalogue_fp_mismatch");
    // The gate would have recorded a version or an alert; the store was never opened.
    assert!(!home.path().join("adapters").exists());
}

#[tokio::test]
async fn unsafe_archives_are_refused() {
    use tar::EntryType::{Char, Link, Regular, Symlink};
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("symlink", raw_archive("top/link", Symlink, Some("/etc/passwd"))),
        ("hard link", raw_archive("top/link", Link, Some("top/other"))),
        ("device", raw_archive("top/dev", Char, None)),
        ("dot-dot", raw_archive("top/../escape", Regular, None)),
        ("absolute", raw_archive("/etc/escape", Regular, None)),
        ("65 entries", archive("top", &(0..65).map(|i| (format!("f{i}"), vec![b'x'])).collect::<Vec<_>>())),
        ("over 256 KiB", archive("top", &[("big".to_owned(), vec![b'x'; 256 * 1024 + 1])])),
    ];
    for (what, bytes) in cases {
        // Through the catalogue, the hash and fingerprint claims are beside the point: the
        // unpack rules run first.
        let mock = serve().await;
        mock.catalogue(bytes.clone(), None, None);
        let home = tempfile::tempdir().unwrap();
        let err = install(&mock, home.path()).await.expect_err(what);
        assert_eq!(err.code(), "catalogue_unpack_refused", "{what}: {err}");
        assert!(unpack::read_archive(&bytes[..]).is_err(), "{what}: the shared unpacker refuses it too");
        assert!(!home.path().join("adapters").exists(), "{what}");
    }
}

#[tokio::test]
async fn http_and_redirects_to_another_host_or_scheme_are_refused() {
    let mock = serve().await;
    mock.catalogue(archive("noop", &noop_files()), None, None);
    let client = mock.client();

    let err = client.fetch_index(&mock.url("/index.toml").replace("https://", "http://")).await.unwrap_err();
    assert_eq!(err.code(), "catalogue_not_https");
    assert_eq!(mock.hits(), 0, "no connection was made");

    mock.route("/other", Reply::Redirect(format!("https://localhost:{}/index.toml", mock.port)));
    let err = client.fetch_index(&mock.url("/other")).await.unwrap_err();
    assert!(err.to_string().contains("another host"), "{err}");

    mock.route("/plain", Reply::Redirect(format!("http://127.0.0.1:{}/index.toml", mock.port)));
    assert_eq!(client.fetch_index(&mock.url("/plain")).await.unwrap_err().code(), "catalogue_fetch_failed");
}

#[tokio::test]
async fn up_to_three_same_host_redirects_are_followed_and_a_fourth_is_not() {
    let mock = serve().await;
    mock.catalogue(archive("noop", &noop_files()), None, None);
    let client = mock.client();
    mock.route("/r3", Reply::Redirect(mock.url("/r2")));
    mock.route("/r2", Reply::Redirect(mock.url("/r1")));
    mock.route("/r1", Reply::Redirect(mock.url("/index.toml")));
    client.fetch_index(&mock.url("/r3")).await.unwrap();
    mock.route("/r4", Reply::Redirect(mock.url("/r3")));
    let err = client.fetch_index(&mock.url("/r4")).await.unwrap_err();
    assert!(err.to_string().contains("redirects"), "{err}");
}

#[tokio::test]
async fn the_size_caps_hold() {
    let mock = serve().await;
    mock.route("/index.toml", Reply::Ok(vec![b'#'; 1024 * 1024 + 1]));
    let err = mock.client().fetch_index(&mock.url("/index.toml")).await.unwrap_err();
    assert_eq!(err.code(), "catalogue_too_large");

    // An archive over 2 MiB, with a hash that would otherwise match.
    let big = vec![b'z'; 2 * 1024 * 1024 + 1];
    mock.catalogue(big.clone(), None, None);
    let home = tempfile::tempdir().unwrap();
    let err = install(&mock, home.path()).await.unwrap_err();
    assert_eq!(err.code(), "catalogue_too_large");
}

#[tokio::test]
async fn a_status_other_than_200_is_a_failed_fetch() {
    let mock = serve().await;
    let err = mock.client().fetch_index(&mock.url("/missing")).await.unwrap_err();
    assert_eq!(err.code(), "catalogue_fetch_failed");
}

#[tokio::test]
async fn an_unlisted_harness_or_version_is_named() {
    let mock = serve().await;
    mock.catalogue(archive("noop", &noop_files()), None, None);
    let home = tempfile::tempdir().unwrap();
    let c = mock.client();
    let url = mock.url("/index.toml");
    let err = c.install(home.path(), &url, "nope", None, opts(&["openai-chat"])).await.unwrap_err();
    assert_eq!(err.code(), "catalogue_unknown_harness");
    let err = c.install(home.path(), &url, "noop", Some("9.9.9"), opts(&["openai-chat"])).await.unwrap_err();
    assert_eq!(err.code(), "catalogue_unknown_version");
}

#[tokio::test]
async fn a_local_archive_goes_through_the_same_unpack_rules() {
    let work = tempfile::tempdir().unwrap();
    let bad = work.path().join("bad.tar.gz");
    std::fs::write(&bad, raw_archive("top/link", tar::EntryType::Symlink, Some("/etc/passwd"))).unwrap();
    let home = tempfile::tempdir().unwrap();
    let err = nullrouter_adapters::install(home.path(), &bad, &opts(&["openai-chat"])).await.unwrap_err();
    assert!(matches!(err, nullrouter_adapters::InstallError::Unpack(_)), "{err}");
}

#[tokio::test]
async fn catalogue_and_local_installs_of_one_source_differ_only_in_origin() {
    let packed = archive("noop-0.1.0", &noop_files());
    let work = tempfile::tempdir().unwrap();
    let local_archive = work.path().join("noop.tar.gz");
    std::fs::write(&local_archive, &packed).unwrap();

    let mock = serve().await;
    mock.catalogue(packed, None, None);
    let (home_c, home_l) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let from_catalogue = install(&mock, home_c.path()).await.unwrap();
    let mut local = opts(&["openai-chat"]);
    local.origin = Origin::Local(local_archive.display().to_string());
    let from_local = nullrouter_adapters::install(home_l.path(), &local_archive, &local).await.unwrap();
    assert_eq!(from_catalogue, from_local);

    let entry = |home: &Path, done: &nullrouter_adapters::Installed| {
        let index = Store::open(home).unwrap().load_index().unwrap();
        let mut v = serde_json::to_value(index.version(&done.harness, &done.version).unwrap()).unwrap();
        let origin = v.as_object_mut().unwrap().remove("origin").unwrap();
        v.as_object_mut().unwrap().remove("submitted");
        (v, origin)
    };
    let (a, origin_a) = entry(home_c.path(), &from_catalogue);
    let (b, origin_b) = entry(home_l.path(), &from_local);
    assert_eq!(a, b);
    assert_ne!(origin_a, origin_b);

    let source = |home: &Path, done: &nullrouter_adapters::Installed| {
        let store = Store::open(home).unwrap();
        let mut files = unpack::read_dir_tree(&store.version_dir(&done.harness, &done.version).join("source")).unwrap();
        files.sort();
        files
    };
    assert_eq!(source(home_c.path(), &from_catalogue), source(home_l.path(), &from_local));
}
