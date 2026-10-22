//! US2 (spec 014, T025): the settings forms through the browser, against the CLI.
//!
//! The CLI side is the shared ops (`nullrouter_server::ops::{routing, connection, proxy, quota}`)
//! called with `Ctx { engine: None }` on a twin home, the dashboard's home copied before any change.
//! The same step runs on both: a refusal must read the same as the CLI's text, and a success must
//! show the CLI's line (the CLI has no server, so its line says "saved; applies at next start"
//! where the dashboard's says "applied"; that one word is the only difference allowed). After all
//! steps the two homes hold the same config, proxies and accounts.

mod common;

use std::path::Path;

use common::{Site, accounts_file, ctx_of, files_under, notice, twin_of};
use nullrouter_dashboard::pages::Id;
use nullrouter_engine::testkit::homes;
use nullrouter_registry::SecretString;
use nullrouter_server::ops::proxy::Target;
use nullrouter_server::ops::{Ctx, Done, OpError, connection, proxy, quota, routing};
use nullrouter_server::views::change_log;
use reqwest::StatusCode;

const SETTINGS: &str = "/settings";
const PROVIDERS_ANTHROPIC: &str = "/providers/anthropic";
const PROXY_POOLS: &str = "/proxy-pools";
const QUOTA: &str = "/quota";

/// The CLI's op on a context, returning its result.
type Cli = fn(&Ctx) -> Result<Done, OpError>;

/// One US2 operation: the form's route, the page it returns to, the fields the form posts, and the
/// CLI's op with the same arguments.
struct Step {
    route: &'static str,
    back: &'static str,
    fields: &'static [(&'static str, &'static str)],
    cli: Cli,
}

/// The steps of US2, in the order an operator would take them. Refusals sit where the state they
/// are refused in is set up.
fn us2_steps() -> Vec<Step> {
    vec![
        Step {
            route: "behaviour/set-break",
            back: SETTINGS,
            fields: &[("behaviour", "error_event")],
            cli: |c| routing::set_break_default(c, "error_event"),
        },
        Step {
            route: "behaviour/set-break",
            back: SETTINGS,
            fields: &[("behaviour", "sometimes")],
            cli: |c| routing::set_break_default(c, "sometimes"),
        },
        Step {
            route: "routing/window",
            back: SETTINGS,
            fields: &[("duration", "2h")],
            cli: |c| routing::window(c, None, "2h"),
        },
        Step {
            route: "routing/window",
            back: SETTINGS,
            fields: &[("duration", "0s")],
            cli: |c| routing::window(c, None, "0s"),
        },
        Step {
            route: "routing/window",
            back: SETTINGS,
            fields: &[("target", "anthropic/claude-sonnet-4-20250514"), ("duration", "1h")],
            cli: |c| routing::window(c, Some("anthropic/claude-sonnet-4-20250514"), "1h"),
        },
        Step {
            route: "routing/set",
            back: PROVIDERS_ANTHROPIC,
            fields: &[("provider", "anthropic"), ("name", "main"), ("key", "reserve"), ("value", "5%")],
            cli: |c| routing::set(c, "anthropic", "main", &["reserve=5%".to_string()]),
        },
        Step {
            route: "routing/set",
            back: PROVIDERS_ANTHROPIC,
            fields: &[("provider", "anthropic"), ("name", "main"), ("key", "reserve"), ("value", "150%")],
            cli: |c| routing::set(c, "anthropic", "main", &["reserve=150%".to_string()]),
        },
        Step {
            route: "routing/unset",
            back: PROVIDERS_ANTHROPIC,
            fields: &[("provider", "anthropic"), ("name", "main"), ("key", "reserve")],
            cli: |c| routing::unset(c, "anthropic", "main", &["reserve".to_string()]),
        },
        Step {
            route: "routing/set-plugin",
            back: PROVIDERS_ANTHROPIC,
            fields: &[("provider", "anthropic"), ("key", "window.5-hour.weight.input"), ("value", "2")],
            cli: |c| routing::set_plugin(c, "anthropic", &["window.5-hour.weight.input=2".to_string()]),
        },
        Step {
            route: "routing/set-plugin",
            back: PROVIDERS_ANTHROPIC,
            fields: &[("provider", "anthropic"), ("key", "window.5-hour.capacity"), ("value", "1")],
            cli: |c| routing::set_plugin(c, "anthropic", &["window.5-hour.capacity=1".to_string()]),
        },
        Step {
            route: "routing/unset-plugin",
            back: PROVIDERS_ANTHROPIC,
            fields: &[("provider", "anthropic"), ("key", "window.5-hour.weight.input")],
            cli: |c| routing::unset_plugin(c, "anthropic", &["window.5-hour.weight.input".to_string()]),
        },
        Step {
            route: "connection/set",
            back: PROVIDERS_ANTHROPIC,
            fields: &[("provider", "anthropic"), ("key", "stall-timeout"), ("value", "30s")],
            cli: |c| connection::set(c, "anthropic", None, "stall-timeout", "30s"),
        },
        Step {
            route: "connection/set",
            back: PROVIDERS_ANTHROPIC,
            fields: &[("provider", "anthropic"), ("key", "connect-timeout"), ("value", "off")],
            cli: |c| connection::set(c, "anthropic", None, "connect-timeout", "off"),
        },
        Step {
            route: "connection/unset",
            back: PROVIDERS_ANTHROPIC,
            fields: &[("provider", "anthropic"), ("key", "stall-timeout")],
            cli: |c| connection::unset(c, "anthropic", None, "stall-timeout"),
        },
        Step {
            route: "proxy/add",
            back: PROXY_POOLS,
            fields: &[
                ("name", "eu"),
                ("url", "http://10.0.0.2:3128"),
                ("username", "bob"),
                ("password", "pw-eu-fixture"),
            ],
            cli: |c| {
                proxy::add(
                    c,
                    &mut Vec::new(),
                    "eu",
                    "http://10.0.0.2:3128",
                    Some("bob"),
                    Some(SecretString::new("pw-eu-fixture")),
                )
            },
        },
        Step {
            route: "proxy/add",
            back: PROXY_POOLS,
            fields: &[("name", "bad"), ("url", "not a url")],
            cli: |c| proxy::add(c, &mut Vec::new(), "bad", "not a url", None, None),
        },
        Step {
            route: "proxy/use",
            back: PROXY_POOLS,
            fields: &[("proxy", "eu"), ("scope", "provider"), ("target", "anthropic")],
            cli: |c| proxy::use_proxy(c, &mut Vec::new(), "eu", &Target::Provider("anthropic".to_string())),
        },
        Step {
            route: "proxy/use",
            back: PROXY_POOLS,
            fields: &[("proxy", "ghost"), ("scope", "all"), ("target", "")],
            cli: |c| proxy::use_proxy(c, &mut Vec::new(), "ghost", &Target::All),
        },
        Step {
            route: "proxy/remove",
            back: PROXY_POOLS,
            fields: &[("name", "eu")],
            cli: |c| proxy::remove(c, &mut Vec::new(), "eu"),
        },
        Step {
            route: "proxy/clear",
            back: PROXY_POOLS,
            fields: &[("scope", "provider"), ("target", "anthropic")],
            cli: |c| proxy::clear(c, &mut Vec::new(), &Target::Provider("anthropic".to_string())),
        },
        Step {
            route: "proxy/remove",
            back: PROXY_POOLS,
            fields: &[("name", "eu")],
            cli: |c| proxy::remove(c, &mut Vec::new(), "eu"),
        },
        Step {
            route: "proxy/remove",
            back: PROXY_POOLS,
            fields: &[("name", "ghost")],
            cli: |c| proxy::remove(c, &mut Vec::new(), "ghost"),
        },
        Step {
            route: "quota/interval",
            back: QUOTA,
            fields: &[("provider", "anthropic"), ("name", "main"), ("every", "15m")],
            cli: |c| quota::interval_with(c, &mut Vec::new(), "anthropic", "main", "15m").map(|r| r.done),
        },
        Step {
            route: "quota/interval",
            back: QUOTA,
            fields: &[("provider", "anthropic"), ("name", "main"), ("every", "soon")],
            cli: |c| quota::interval_with(c, &mut Vec::new(), "anthropic", "main", "soon").map(|r| r.done),
        },
        Step {
            route: "quota/ack",
            back: QUOTA,
            fields: &[("id", "all")],
            cli: |c| quota::ack_with(c, "all", None, None).map(|r| r.done),
        },
    ]
}

/// A home with one valid account (no invalid plugin, so a reload succeeds): the fixture's
/// `anthropic/main`, with the same secret.
fn base_home() -> tempfile::TempDir {
    let dir = homes::empty();
    let accounts = "schema = 2\n\n[[account]]\nprovider = \"anthropic\"\nname = \"main\"\nsecret = \"sk-fixture-main-AAAA0001\"\norder = 1\n";
    let file = dir.path().join(nullrouter_engine::accounts::FILE);
    std::fs::write(&file, accounts).unwrap();
    // The engine refuses a secret file that other users can read.
    std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    dir
}

/// The CLI's line with the one word a server-less CLI differs on.
fn cli_line(d: &Done) -> String {
    d.text.replace("saved; applies at next start", "applied")
}

/// `config.toml` and `proxies.toml` as text; empty when the home has none.
fn text_file(home: &Path, name: &str) -> String {
    std::fs::read_to_string(home.join(name)).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn every_us2_op_through_the_browser_leaves_the_same_state_as_the_cli() {
    let site = Site::start_on(base_home()).await;
    let twin = twin_of(site.home());
    let mut refusals = 0;
    for step in us2_steps() {
        let fields: Vec<(&str, String)> = step.fields.iter().map(|(k, v)| (*k, (*v).to_owned())).collect();
        let flash = site.flow(step.route, step.back, &fields).await;
        let (is_error, shown) = notice(&flash.page).expect("the flash notice");
        match (step.cli)(&ctx_of(twin.path())) {
            Err(e) => {
                refusals += 1;
                assert!(is_error, "{}: the CLI refused, the page did not: {shown}", step.route);
                assert_eq!(shown, e.text, "{} {:?}: the page and the CLI disagree", step.route, step.fields);
            }
            Ok(d) => {
                assert!(!is_error, "{}: the page refused, the CLI did not: {shown}", step.route);
                assert_eq!(shown, cli_line(&d), "{} {:?}: the page and the CLI disagree", step.route, step.fields);
            }
        }
    }
    // The comparison must cover refusals and successes alike.
    assert!(refusals > 0 && refusals < us2_steps().len(), "{refusals} refusals");

    assert_eq!(text_file(site.home(), "config.toml"), text_file(twin.path(), "config.toml"), "config.toml differs");
    assert_eq!(
        text_file(site.home(), nullrouter_engine::connection::proxy::FILE),
        text_file(twin.path(), nullrouter_engine::connection::proxy::FILE),
        "proxies differ"
    );
    assert_eq!(accounts_file(site.home()), accounts_file(twin.path()), "accounts.toml differs");
    // The steps left work behind, so the comparison did not pass on untouched files.
    assert!(text_file(site.home(), "config.toml").contains("break_behaviour = \"error_event\""));
    assert!(text_file(site.home(), "config.toml").contains("amortization = \"2h\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_out_of_range_override_shows_the_clis_text() {
    let site = Site::start_on(base_home()).await;
    let twin = twin_of(site.home());
    let fields = [
        ("provider", "anthropic".to_owned()),
        ("name", "main".to_owned()),
        ("key", "reserve".to_owned()),
        ("value", "150%".to_owned()),
    ];
    let flash = site.flow("routing/set", PROVIDERS_ANTHROPIC, &fields).await;
    let refused = flash.refused.expect("an override of 150% is refused");
    let cli = routing::set(&ctx_of(twin.path()), "anthropic", "main", &["reserve=150%".to_string()])
        .expect_err("the CLI refuses 150%");
    assert_eq!(refused, cli.text);
    assert!(refused.contains("reserve"), "{refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_refuses_the_reload_reads_saved_but_not_applied_with_the_reason() {
    let site = Site::start_on(base_home()).await;
    // A plugin the reload cannot load, written after the server opened: the change is saved, the
    // running server keeps its previous state, and the page says why.
    let plugins = site.home().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::write(plugins.join("zz-reload.toml"), "schema = 2\nid = \"zz-reload\"\nnot a plugin\n").unwrap();

    let flash = site.flow("behaviour/set-break", SETTINGS, &[("behaviour", "error_event".to_owned())]).await;
    let (is_error, shown) = notice(&flash.page).expect("the flash notice");
    assert!(!is_error, "a saved change is not a refusal: {shown}");
    // The reason is said once: the server's own text is the reason, and the flash adds the label.
    assert!(
        shown.starts_with("saved but not applied: saved, but the running server kept its previous state: "),
        "{shown}"
    );
    assert_eq!(shown.matches("kept its previous state").count(), 1, "the reason is repeated: {shown}");
    assert!(shown.len() > "saved but not applied: saved, but the running server kept its previous state: ".len());
    // The change itself is on disk.
    assert!(text_file(site.home(), "config.toml").contains("break_behaviour = \"error_event\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proxy_password_marker_is_in_no_response_and_no_file_but_the_proxy_store() {
    let site = Site::start_on(base_home()).await;
    let mut raw = [0u8; 8];
    getrandom::fill(&mut raw).unwrap();
    let marker = format!("nrmarker-{}", raw.iter().map(|b| format!("{b:02x}")).collect::<String>());

    let fields = [
        ("name", "marked".to_owned()),
        ("url", "http://10.0.0.3:3128".to_owned()),
        ("username", "carol".to_owned()),
        ("password", marker.clone()),
    ];
    let flash = site.flow("proxy/add", PROXY_POOLS, &fields).await;
    assert_eq!(flash.refused, None, "the marked proxy was refused");

    for id in Id::ALL {
        let (status, _) = site.get(id.path()).await;
        assert_eq!(status, StatusCode::OK, "{}", id.path());
    }
    site.get(PROXY_POOLS).await;
    site.get(PROVIDERS_ANTHROPIC).await;

    for body in site.seen.lock().unwrap().iter() {
        assert!(!body.contains(&marker), "a response carries the password");
    }

    let holding: Vec<String> = files_under(site.home())
        .into_iter()
        .filter(|f| std::fs::read(site.home().join(f)).unwrap().windows(marker.len()).any(|w| w == marker.as_bytes()))
        .collect();
    assert_eq!(holding, [nullrouter_engine::connection::proxy::FILE.to_owned()], "the password is in another file");
    let log = std::fs::read_to_string(change_log::path(&site.d.home())).unwrap();
    assert!(!log.contains(&marker), "the change log holds the password");
}

#[tokio::test(flavor = "multi_thread")]
async fn proxy_fixed_and_quota_poll_answer_with_a_refusal_never_a_server_error() {
    // Both start network work when they name a real proxy or account, so the posts here name ones
    // that do not exist: each must answer 303 with a refusal flash, never a 500.
    let site = Site::start_on(base_home()).await;
    for (route, back, fields) in [
        ("proxy/fixed", PROXY_POOLS, vec![("name", "ghost".to_owned())]),
        ("quota/poll", QUOTA, vec![("provider", "anthropic".to_owned()), ("name", "ghost".to_owned())]),
    ] {
        let r = site.post(route, back, &fields.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>()).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER, "{route}");
        let location = r.headers()[reqwest::header::LOCATION].to_str().unwrap().to_owned();
        assert!(location.contains("done=") || location.contains("task="), "{route} went to {location}");
        let (status, page) = site.get(&location).await;
        assert_eq!(status, StatusCode::OK, "{route}");
        let (is_error, shown) = notice(&page).unwrap_or_else(|| panic!("{route}: no flash on {location}"));
        assert!(is_error, "{route}: expected a refusal, got {shown}");
        assert!(shown.contains("ghost"), "{route}: {shown}");
    }
}
