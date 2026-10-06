//! User Story 8 (T060, contracts/plugin-logo.md): a plugin ships its logo. A user plugin whose
//! logo breaks a limit loads and serves a request, its card shows the text icon, and `check` names
//! the plugin and the reason; a good logo is served under its content hash, behind the cookie, as
//! an image only; `plugins install` and `uninstall` copy and remove a community plugin's logo.

mod common;

use std::fs;
use std::path::Path;
use std::time::Duration;

use common::{Dash, TOKEN};
use nullrouter_dashboard::headers::IMMUTABLE;
use nullrouter_dashboard::logos::Index;
use nullrouter_engine::accounts;
use nullrouter_engine::files::write_private;
use nullrouter_engine::keys::{self, Keys};
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_registry::community;
use nullrouter_registry::{OperatorHome, RegistryHandle};
use nullrouter_server::views::{self, Live};
use reqwest::header::{CACHE_CONTROL, CONTENT_TYPE, COOKIE, LOCATION, X_CONTENT_TYPE_OPTIONS};
use reqwest::{Client, StatusCode, redirect};
use serde_json::{Value, json};

/// A PNG header of `width` × `height` (signature, then `IHDR`; the CRC isn't checked), padded with
/// zeros to `len` bytes. The core never decodes a logo, so this is all it reads.
fn png(width: u32, height: u32, len: usize) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
    b.extend_from_slice(&width.to_be_bytes());
    b.extend_from_slice(&height.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    b.resize(len.max(b.len()), 0);
    b
}

/// The start of a JPEG (JFIF) file, padded: what 9router's `nebius.png` really is.
fn jpeg() -> Vec<u8> {
    let mut b = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0];
    b.resize(4096, 0);
    b
}

const GOOD: &str = "logo-good";

/// Each bad case: the plugin id, its logo file (`None`: no file), and the reason `check` gives.
fn bad() -> Vec<(&'static str, Option<Vec<u8>>, &'static str)> {
    vec![
        ("logo-big", Some(png(64, 64, 70_000)), "69 KiB, over 64 KiB"),
        ("logo-wide", Some(png(257, 10, 512)), "257 × 10 px, over 256 px"),
        ("logo-jpeg", Some(jpeg()), "not a PNG"),
        ("logo-missing", None, "file not found: logos/logo-missing.png"),
    ]
}

/// A user plugin `id` whose one text endpoint is the mock, declaring `logo = "<id>.png"`.
fn plugin(mock: &MockUpstream, id: &str) -> String {
    format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\nlogo = \"{id}.png\"\n\n[auth]\nkind = \"apikey\"\n\
         header = \"Authorization\"\nscheme = \"bearer\"\n\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n\n\
         [[models]]\nid = \"m1\"\n",
        mock.url(&format!("/{id}/chat/completions"))
    )
}

/// A home with the good plugin and every bad one, each with an account, and one agent key.
fn home(mock: &MockUpstream, good: &[u8]) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    fs::create_dir_all(h.join("plugins/logos")).unwrap();
    fs::write(h.join("config.toml"), "allow_private_endpoints = true\n").unwrap();
    let mut cases: Vec<(&str, Option<Vec<u8>>)> = vec![(GOOD, Some(good.to_vec()))];
    cases.extend(bad().into_iter().map(|(id, bytes, _)| (id, bytes)));
    let mut accounts = String::from("schema = 1\n");
    for (id, bytes) in cases {
        fs::write(h.join(format!("plugins/{id}.toml")), plugin(mock, id)).unwrap();
        if let Some(bytes) = bytes {
            fs::write(h.join(format!("plugins/logos/{id}.png")), bytes).unwrap();
        }
        accounts += &format!("[[account]]\nprovider = \"{id}\"\nname = \"main\"\nsecret = \"sk-{id}-SENTINEL\"\n");
    }
    write_private(&h.join(accounts::FILE), &accounts).unwrap();
    let mut keys = Keys::default();
    let (key, _) = keys.issue("laptop", None).unwrap();
    write_private(&h.join(keys::FILE), &keys.to_toml()).unwrap();
    (dir, key)
}

fn client() -> Client {
    Client::builder().redirect(redirect::Policy::none()).no_proxy().timeout(Duration::from_secs(30)).build().unwrap()
}

/// The provider card for `id` on the Providers page: from its link to the end of the link.
fn card<'a>(html: &'a str, id: &str) -> &'a str {
    let start = html
        .find(&format!("href=\"/providers/{id}\""))
        .or_else(|| html.find(&format!("href=\"/providers/{id}?")))
        .unwrap_or_else(|| panic!("no card for {id}"));
    let rest = &html[start..];
    &rest[..rest.find("</a>").unwrap_or(rest.len())]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_logo_is_ignored_and_a_good_one_is_served_as_an_image() {
    let mock = MockUpstream::start().await;
    mock.respond(|r| {
        let content = format!("hi from {}", r.path_and_query);
        Step::json(
            200,
            json!({"id": "up-1", "object": "chat.completion", "created": 1, "model": "m1",
                   "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
                   "usage": {"prompt_tokens": 3, "completion_tokens": 3}}),
        )
    });
    let good = png(64, 48, 2048);
    let (dir, key) = home(&mock, &good);
    let d = Dash::start(dir).await;

    // `check`: one note per bad logo, on Providers, in the words the CLI prints; none for the good.
    let check = views::check::build(&d.home(), &json!({}), &Live::none()).unwrap().json;
    let ignored = check["logos_ignored"].as_array().unwrap();
    assert_eq!(ignored.len(), bad().len(), "{ignored:#?}");
    let notices = check["notices"].as_array().unwrap();
    for (id, _, reason) in bad() {
        assert!(ignored.contains(&json!({"id": id, "reason": reason})), "{id}: {ignored:#?}");
        let text = format!("note: logo ignored: {id}: {reason}");
        let n = notices.iter().find(|n| n["text"] == text.as_str()).unwrap_or_else(|| panic!("{text}: {notices:#?}"));
        assert_eq!((n["level"].as_str(), n["subject"].as_str()), (Some("note"), Some("providers")), "{n}");
    }
    assert!(!check.to_string().contains("logo ignored: logo-good"), "{check:#}");

    // Every plugin loads and serves a request, its logo or not.
    let http = client();
    for id in std::iter::once(GOOD).chain(bad().into_iter().map(|(id, _, _)| id)) {
        let r = http
            .post(format!("http://{}/v1/chat/completions", d.client_addr))
            .bearer_auth(&key)
            .body(json!({"model": format!("{id}/m1"), "messages": [{"role": "user", "content": "hi"}]}).to_string())
            .send()
            .await
            .unwrap();
        let status = r.status();
        let body: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
        assert_eq!(status, StatusCode::OK, "{id}: {body}");
        assert_eq!(body["choices"][0]["message"]["content"], format!("hi from /{id}/chat/completions"), "{id}");
    }

    // The page: the good logo by its address, a text icon on each bad plugin's card.
    let index = Index::of(&d.engine);
    let href = index.href(GOOD).expect("the good logo has an address").to_owned();
    assert!(href.starts_with("/logos/") && href.ends_with("/logo-good.png"), "{href}");
    let html = d.ok("/providers").await;
    assert!(card(&html, GOOD).contains(&format!("src=\"{href}\"")), "{}", card(&html, GOOD));
    for (id, _, _) in bad() {
        assert_eq!(index.href(id), None, "{id}");
        let c = card(&html, id);
        assert!(c.contains("text-icon") && !c.contains("<img"), "{id}: {c}");
        // The check note above the page names the file; no address serves it.
        assert!(!html.contains(&format!("/{id}.png\"")), "{id}: no logo address on the page");
    }

    // Served as an image only, cacheable under its hash, and only to a signed-in browser.
    let get = |path: &str, cookie: bool| {
        let req = http.get(format!("http://{}{path}", d.addr));
        if cookie { req.header(COOKIE, format!("nr_dashboard={TOKEN}")) } else { req }
    };
    let r = get(&href, true).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[CONTENT_TYPE], "image/png");
    assert_eq!(r.headers()[X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(r.headers()[CACHE_CONTROL], IMMUTABLE);
    assert_eq!(r.bytes().await.unwrap().as_ref(), &good[..]);

    let r = get(&href, false).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(r.headers()[LOCATION].to_str().unwrap().starts_with("/signin"), "{:?}", r.headers()[LOCATION]);

    let hash = href.trim_start_matches("/logos/").split('/').next().unwrap().to_owned();
    for wrong in [
        "/logos/0000000000000000/logo-good.png".to_owned(),
        format!("/logos/{hash}/logo-jpeg.png"),
        format!("/logos/{hash}/logo-missing.png"),
        format!("/logos/{hash}/logo-good.svg"),
        format!("/logos/{hash}/../logo-good.png"),
    ] {
        let r = get(&wrong, true).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{wrong}");
        assert_ne!(r.headers()[CONTENT_TYPE], "image/png", "{wrong}");
    }

    // A changed logo gets a new address once the home is reloaded; the old one stops answering.
    fs::write(d.dir.path().join("plugins/logos/logo-good.png"), png(32, 32, 1024)).unwrap();
    d.engine.reload().await.unwrap();
    let moved = Index::of(&d.engine).href(GOOD).unwrap().to_owned();
    assert_ne!(moved, href);
    assert_eq!(get(&href, true).send().await.unwrap().status(), StatusCode::NOT_FOUND);
    assert_eq!(get(&moved, true).send().await.unwrap().status(), StatusCode::OK);
}

/// The bundled plugins' logos are embedded; `opencode-zen` has none and shows its text icon.
#[test]
fn bundled_logos_are_embedded() {
    let dir = tempfile::tempdir().unwrap();
    let reg = RegistryHandle::open(OperatorHome::new(dir.path())).unwrap().snapshot();
    assert!(reg.report().logos_ignored.is_empty(), "{:#?}", reg.report().logos_ignored);
    let index = Index::of_registry(&reg);
    for id in ["anthropic", "elevenlabs", "grok-cli", "opencode-go", "openrouter", "xai"] {
        let logo = reg.logo(id).unwrap_or_else(|| panic!("{id} has a logo"));
        assert_eq!(index.href(id), Some(format!("/logos/{}/{id}.png", logo.hash()).as_str()));
        assert!(logo.bytes().len() <= 65_536, "{id}");
    }
    assert!(reg.logo("opencode-zen").is_none() && index.href("opencode-zen").is_none());
}

/// `plugins install` copies a community plugin's logo to `<home>/plugins/logos/`, where the load
/// finds it; `plugins uninstall` removes it with the plugin.
#[test]
fn install_and_uninstall_copy_and_remove_a_community_logo() {
    let dir = tempfile::tempdir().unwrap();
    let home = OperatorHome::new(dir.path());
    let groq = community::community().iter().find(|p| p.id == "groq").unwrap();
    assert_eq!(groq.logo().as_deref(), Some("groq.png"));
    let logo = dir.path().join("plugins/logos/groq.png");

    community::install("groq", &home).unwrap();
    let bytes = fs::read(&logo).expect("install copies the logo");
    let reg = RegistryHandle::open(home.clone()).unwrap().snapshot();
    assert!(reg.report().logos_ignored.is_empty(), "{:#?}", reg.report().logos_ignored);
    assert_eq!(reg.logo("groq").map(|l| l.bytes()), Some(&bytes[..]));
    assert!(no_temporary_files(&dir.path().join("plugins")), "the copies are atomic");

    community::uninstall("groq", &home).unwrap();
    assert!(!logo.exists(), "uninstall removes the logo");
    assert!(!dir.path().join("plugins/groq.toml").exists());
    let reg = RegistryHandle::open(home).unwrap().snapshot();
    assert!(reg.logo("groq").is_none());
}

fn no_temporary_files(dir: &Path) -> bool {
    let names = |d: &Path| -> Vec<String> {
        fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
    };
    names(dir).iter().chain(names(&dir.join("logos")).iter()).all(|n| !n.ends_with(".tmp"))
}
