//! The confirm sweep (spec 014 SC-007): for every irreversible op, its button leads to the confirm
//! window, the window says what is lost, Cancel and a POST without `confirmed=on` leave every home
//! file byte-identical (the dashboard log aside, since it records refusals), and a POST with
//! `confirmed=on` applies the change.
//!
//! The cases iterate `nullrouter_server::ops::IRREVERSIBLE`, so an op added there without a case
//! fails `every_irreversible_op_has_a_case`.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{Site, attrs, files_under, text_of};
use nullrouter_dashboard::page::ViewName;
use nullrouter_engine::accounts;
use nullrouter_engine::keys;
use nullrouter_engine::testkit::homes;
use nullrouter_server::ops::{IRREVERSIBLE, describe_loss};
use nullrouter_server::views::change_log;
use reqwest::StatusCode;
use serde_json::json;

const ADAPTERS: &str = "adapters/index.toml";
const CONFIG: &str = "config.toml";
const PROXIES: &str = "proxies.toml";
const PLUGIN: &str = "plugins/baidu.toml";

/// Where the button of an op is, and how it opens the window.
#[derive(Clone, Copy)]
enum Button {
    /// A link on `page` whose target carries every mark.
    Link { page: &'static str, marks: &'static [&'static str] },
    /// The GET form on `page` that names the subject: it opens `open`, the window of `window`.
    Form { page: &'static str, window: &'static str, open: &'static str },
}

impl Button {
    fn page(self) -> &'static str {
        match self {
            Button::Link { page, .. } | Button::Form { page, .. } => page,
        }
    }
}

/// What a confirmed change leaves behind.
#[derive(Clone, Copy)]
enum Effect {
    /// The change holds on the home's files.
    Holds(fn(&Path) -> bool),
    /// Private endpoints are on in the running dashboard's view.
    TurnedOn,
}

/// One irreversible op, its subject and the form its window posts.
#[derive(Clone, Copy)]
struct Case {
    op: &'static str,
    /// The subject `describe_loss` names in the window.
    subject: &'static str,
    /// Writes the subject into a fresh home.
    seed: fn(&Path),
    button: Button,
    /// Where the form returns (`_back`).
    back: &'static str,
    /// The route the form posts to, under `/do/`.
    route: &'static str,
    /// The fields the window's form posts without the confirmation.
    base: &'static [(&'static str, &'static str)],
    /// The fields added when the operator confirms.
    confirm: &'static [(&'static str, &'static str)],
    effect: Effect,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            op: "accounts.remove",
            subject: "anthropic/spare",
            seed: seed_accounts,
            button: Button::Link {
                page: "/providers/anthropic",
                marks: &["window=remove-account", "id=anthropic", "name=spare"],
            },
            back: "/providers/anthropic",
            route: "accounts/remove",
            base: &[("_window", "remove-account"), ("provider", "anthropic"), ("name", "spare")],
            confirm: &[("confirmed", "on")],
            effect: Effect::Holds(spare_gone),
        },
        Case {
            op: "keys.revoke",
            subject: "ak_fixture1",
            seed: seed_keys,
            button: Button::Link { page: "/endpoint", marks: &["window=revoke", "key=ak_fixture1"] },
            back: "/endpoint",
            route: "keys/revoke",
            base: &[("_window", "revoke"), ("key", "ak_fixture1")],
            confirm: &[("confirmed", "on")],
            effect: Effect::Holds(laptop_revoked),
        },
        Case {
            op: "plugins.uninstall",
            subject: "baidu",
            seed: seed_plugin,
            button: Button::Link { page: "/providers", marks: &["window=uninstall-plugin", "id=baidu"] },
            back: "/providers",
            route: "plugins/uninstall",
            base: &[("_window", "uninstall-plugin"), ("id", "baidu")],
            confirm: &[("confirmed", "on")],
            effect: Effect::Holds(plugin_gone),
        },
        Case {
            op: "proxy.remove",
            subject: "office",
            seed: seed_proxy,
            button: Button::Form {
                page: "/proxy-pools",
                window: "remove",
                open: "/proxy-pools?window=remove&name=office",
            },
            back: "/proxy-pools",
            route: "proxy/remove",
            base: &[("_window", "remove"), ("name", "office")],
            confirm: &[("confirmed", "on")],
            effect: Effect::Holds(proxy_gone),
        },
        Case {
            op: "adapters.remove",
            subject: "claude-code v1.2.0-cccccccc",
            seed: seed_adapters,
            button: Button::Link { page: "/endpoint", marks: &["window=remove-adapter", "version=v1.2.0-cccccccc"] },
            back: "/endpoint",
            route: "adapters/remove",
            base: &[("_window", "remove-adapter"), ("harness", "claude-code"), ("version", "v1.2.0-cccccccc")],
            confirm: &[("confirmed", "on")],
            effect: Effect::Holds(rejected_version_gone),
        },
        Case {
            op: "adapters.clear",
            subject: "claude-code v1.0.0-aaaaaaaa",
            seed: seed_adapters,
            button: Button::Link { page: "/endpoint", marks: &["window=clear-adapter", "version=v1.0.0-aaaaaaaa"] },
            back: "/endpoint",
            route: "adapters/clear",
            base: &[("_window", "clear-adapter"), ("harness", "claude-code"), ("version", "v1.0.0-aaaaaaaa")],
            confirm: &[("confirmed", "on")],
            effect: Effect::Holds(suspect_version_approved),
        },
        Case {
            op: "unified.remove",
            subject: "u",
            seed: seed_unified,
            button: Button::Link { page: "/combo", marks: &["window=remove-unified", "name=u"] },
            back: "/combo",
            route: "unified/remove",
            base: &[("_window", "remove-unified"), ("name", "u")],
            confirm: &[("confirmed", "on")],
            effect: Effect::Holds(unified_gone),
        },
        Case {
            op: "combos.remove",
            subject: "c",
            seed: seed_combo,
            button: Button::Link { page: "/combo", marks: &["window=remove-combo", "name=c"] },
            back: "/combo",
            route: "combos/remove",
            base: &[("_window", "remove-combo"), ("name", "c")],
            confirm: &[("confirmed", "on")],
            effect: Effect::Holds(combo_gone),
        },
        Case {
            op: "plugins.private_endpoints",
            subject: "on",
            seed: seed_nothing,
            button: Button::Link { page: "/settings", marks: &["window=private-endpoints"] },
            back: "/settings",
            route: "plugins/private-endpoints",
            base: &[("_window", "private-endpoints"), ("on", "on")],
            confirm: &[("confirmed", "on")],
            effect: Effect::TurnedOn,
        },
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn every_irreversible_op_has_a_case() {
    let cases = cases();
    for op in IRREVERSIBLE {
        assert!(cases.iter().any(|c| c.op == *op), "{op} has no confirm case in this sweep");
    }
    assert_eq!(cases.len(), IRREVERSIBLE.len(), "a case for an op that is not irreversible");
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_and_refusal_leave_the_home_and_confirm_applies_for_every_irreversible_op() {
    for c in cases() {
        check(&c).await;
    }
}

/// One op through the sweep: the button, the window, Cancel, a POST without the confirmation, and
/// the POST with it.
async fn check(c: &Case) {
    let op = c.op;
    let dir = homes::empty();
    (c.seed)(dir.path());
    let site = Site::start_on(dir).await;
    site.auto_confirm.store(false, std::sync::atomic::Ordering::Relaxed);
    let before = snapshot(site.home());

    // The button on its page opens the window.
    let (status, page) = site.get(c.button.page()).await;
    assert_eq!(status, StatusCode::OK, "{op}: {}: {}", c.button.page(), text_of(&page));
    let open = match c.button {
        Button::Link { marks, .. } => {
            link_with(&page, marks).unwrap_or_else(|| panic!("{op}: no link on {} opens its window", c.button.page()))
        }
        Button::Form { window, open, .. } => {
            assert!(
                page.contains(&format!("name=\"window\" value=\"{window}\"")),
                "{op}: no {window} form on the page"
            );
            open.to_owned()
        }
    };

    // The window opens, names what is lost, and posts to the op.
    let (status, window) = site.get(&open).await;
    assert_eq!(status, StatusCode::OK, "{op}: {open}");
    assert!(window.contains("class=\"modal\""), "{op}: {open} opens no window");
    let loss = describe_loss(op, c.subject).expect("an irreversible op has a loss text");
    assert!(
        text_of(&window).contains(&loss),
        "{op}: the window does not say what is lost: {loss}\n{}",
        text_of(&window)
    );
    assert!(window.contains(&format!("action=\"/do/{}\"", c.route)), "{op}: the window posts to /do/{}", c.route);

    // Cancel, then a POST without the confirmation: neither changes a file.
    let cancel = cancel_href(&window);
    let (status, _) = site.get(&cancel).await;
    assert_eq!(status, StatusCode::OK, "{op}: Cancel goes to {cancel}");
    assert_eq!(snapshot(site.home()), before, "{op}: Cancel changed the home");

    let refused = site.post(c.route, c.back, &fields(c.base, &[])).await;
    assert_eq!(refused.status(), StatusCode::SEE_OTHER, "{op}: an unconfirmed POST is a redirect");
    let to = refused.headers()[reqwest::header::LOCATION].to_str().unwrap().to_owned();
    assert!(
        to.starts_with(&format!("/{}", c.back.trim_start_matches('/').split(['/', '?']).next().unwrap_or_default()))
            && to.contains("window="),
        "{op}: the refusal goes to {to}, not to a confirm window"
    );
    let (status, again) = site.get(&to).await;
    assert_eq!(status, StatusCode::OK, "{op}: {to}");
    assert!(again.contains("class=\"modal\""), "{op}: {to} opens no window");
    assert_eq!(snapshot(site.home()), before, "{op}: a POST without confirmed=on changed the home");

    // The confirmed POST applies the change.
    let flash = site.flow(c.route, c.back, &fields(c.base, c.confirm)).await;
    assert_eq!(flash.refused, None, "{op}: the confirmed change was refused");
    match c.effect {
        Effect::Holds(holds) => assert!(holds(site.home()), "{op}: confirmed, but the change did not apply"),
        Effect::TurnedOn => {
            let view = site.d.view(ViewName::Check, json!({})).await;
            assert_eq!(view["allow_private_endpoints"], json!(true), "{op}: confirmed, but private endpoints are off");
        }
    }
}

/// The form fields of one POST: the window's own, then the confirmation.
fn fields(
    base: &[(&'static str, &'static str)],
    extra: &[(&'static str, &'static str)],
) -> Vec<(&'static str, String)> {
    base.iter().chain(extra.iter()).map(|(k, v)| (*k, (*v).to_owned())).collect()
}

/// Every file under the home, as its bytes, the dashboard log aside.
fn snapshot(home: &Path) -> BTreeMap<String, Vec<u8>> {
    files_under(home)
        .into_iter()
        .filter(|f| f.as_str() != change_log::FILE)
        .map(|f| {
            let bytes = std::fs::read(home.join(&f)).unwrap();
            (f, bytes)
        })
        .collect()
}

/// The `href` of the first link on `html` whose target carries every mark.
fn link_with(html: &str, marks: &[&str]) -> Option<String> {
    attrs(html, "href").into_iter().find(|h| marks.iter().all(|m| h.contains(m)))
}

/// The `href` of the Cancel link of a confirm window.
fn cancel_href(window: &str) -> String {
    let at = window.find(">Cancel</a>").expect("the window has a Cancel link");
    let start = window[..at].rfind("<a ").expect("Cancel is a link");
    attrs(&window[start..at], "href").into_iter().next().expect("the Cancel link has an href")
}

/// The tables of an array of tables.
fn tables(v: Option<&toml::Value>) -> Vec<toml::Table> {
    v.and_then(toml::Value::as_array).into_iter().flatten().filter_map(|v| v.as_table().cloned()).collect()
}

/// The tables under `table` in `file` of the home; none when the file is missing.
fn rows(home: &Path, file: &str, table: &str) -> Vec<toml::Table> {
    let doc = match std::fs::read_to_string(home.join(file)) {
        Ok(text) => toml::from_str::<toml::Table>(&text).unwrap(),
        Err(_) => toml::Table::new(),
    };
    tables(doc.get(table))
}

/// The string under `key` of a table.
fn text(row: &toml::Table, key: &str) -> Option<String> {
    row.get(key)?.as_str().map(str::to_owned)
}

/// The versions of the `claude-code` harness in the adapter index.
fn versions(home: &Path) -> Vec<toml::Table> {
    rows(home, ADAPTERS, "harness")
        .iter()
        .filter(|h| text(h, "name").as_deref() == Some("claude-code"))
        .flat_map(|h| tables(h.get("version")))
        .collect()
}

/// Writes `text` to `rel` under `home`, making its directory.
fn put_text(home: &Path, rel: &str, text: &str) {
    let path = home.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // The stores refuse a directory or a file others can read.
    use std::os::unix::fs::PermissionsExt;
    if path.parent() != Some(home) {
        std::fs::set_permissions(path.parent().unwrap(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    nullrouter_engine::files::write_private(&path, text).unwrap();
}

fn seed_nothing(_home: &Path) {}

fn seed_accounts(home: &Path) {
    put_text(
        home,
        accounts::FILE,
        "schema = 2\n\n[[account]]\nprovider = \"anthropic\"\nname = \"spare\"\nsecret = \"sk-fixture-spare-BBBB0002\"\norder = 2\n",
    );
}

fn seed_keys(home: &Path) {
    put_text(
        home,
        keys::FILE,
        "schema = 1\n\n[[key]]\nid = \"ak_fixture1\"\nname = \"laptop\"\n\
         digest = \"0000000000000000000000000000000000000000000000000000000000000001\"\n\
         last4 = \"L0P1\"\ncreated = \"2026-10-01T08:00:00Z\"\n",
    );
}

fn seed_plugin(home: &Path) {
    let source = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/community/baidu.toml");
    std::fs::create_dir_all(home.join("plugins")).unwrap();
    std::fs::copy(source, home.join(PLUGIN)).unwrap();
}

fn seed_proxy(home: &Path) {
    put_text(home, PROXIES, "schema = 1\n\n[[proxy]]\nname = \"office\"\nurl = \"http://127.0.0.1:3128\"\n");
}

/// Two `claude-code` versions: `v1.0.0-aaaaaaaa` is suspect (the clear), `v1.2.0-cccccccc` is
/// rejected and not active (the remove).
fn seed_adapters(home: &Path) {
    let fp = |c: char| format!("sha256:{}", c.to_string().repeat(64));
    put_text(
        home,
        ADAPTERS,
        &format!(
            "schema = 1\n\n[[harness]]\nname = \"claude-code\"\n\n\
             [[harness.version]]\nid = \"v1.0.0-aaaaaaaa\"\nsemver = \"1.0.0\"\nstate = \"suspect\"\n\
             source_fp = \"{}\"\nsubmitted = \"2026-10-01T08:00:00Z\"\norigin = {{ local = \"claude-code\" }}\n\n\
             [[harness.version]]\nid = \"v1.2.0-cccccccc\"\nsemver = \"1.2.0\"\nstate = \"rejected\"\n\
             source_fp = \"{}\"\nsubmitted = \"2026-10-02T08:00:00Z\"\norigin = {{ local = \"claude-code\" }}\n",
            fp('a'),
            fp('c'),
        ),
    );
}

fn seed_unified(home: &Path) {
    put_text(
        home,
        CONFIG,
        "schema = 1\n\n[[unified_model]]\nname = \"u\"\nmembers = [\n  { provider = \"xai\", model = \"grok-4\" },\n]\n",
    );
}

/// A unified model `u` and a combo `c` that lists it: removing `c` is allowed, removing `u` is not.
fn seed_combo(home: &Path) {
    put_text(
        home,
        CONFIG,
        "schema = 1\n\n[[unified_model]]\nname = \"u\"\nmembers = [\n  { provider = \"xai\", model = \"grok-4\" },\n]\n\n\
         [[combo]]\nname = \"c\"\nmembers = [\"u\"]\n",
    );
}

fn spare_gone(home: &Path) -> bool {
    !rows(home, accounts::FILE, "account")
        .iter()
        .any(|r| text(r, "provider").as_deref() == Some("anthropic") && text(r, "name").as_deref() == Some("spare"))
}

fn laptop_revoked(home: &Path) -> bool {
    rows(home, keys::FILE, "key")
        .iter()
        .any(|r| text(r, "id").as_deref() == Some("ak_fixture1") && r.contains_key("revoked"))
}

fn plugin_gone(home: &Path) -> bool {
    !home.join(PLUGIN).exists()
}

fn proxy_gone(home: &Path) -> bool {
    !rows(home, PROXIES, "proxy").iter().any(|r| text(r, "name").as_deref() == Some("office"))
}

fn rejected_version_gone(home: &Path) -> bool {
    !versions(home).iter().any(|v| text(v, "id").as_deref() == Some("v1.2.0-cccccccc"))
}

fn suspect_version_approved(home: &Path) -> bool {
    versions(home)
        .iter()
        .any(|v| text(v, "id").as_deref() == Some("v1.0.0-aaaaaaaa") && text(v, "state").as_deref() == Some("approved"))
}

fn unified_gone(home: &Path) -> bool {
    !rows(home, CONFIG, "unified_model").iter().any(|r| text(r, "name").as_deref() == Some("u"))
}

fn combo_gone(home: &Path) -> bool {
    !rows(home, CONFIG, "combo").iter().any(|r| text(r, "name").as_deref() == Some("c"))
}
