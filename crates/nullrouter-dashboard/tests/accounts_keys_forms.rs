//! US1 (spec 014, T018): the account and key forms through the browser, against the CLI.
//!
//! The CLI side is the shared ops (`nullrouter_server::ops::{accounts, keys}`) called with
//! `Ctx { engine: None }` on a twin home: the dashboard's home copied before any change, so both
//! start from the same files. The same steps run on both; each refusal must read the same, and the
//! two homes must hold the same accounts and keys afterwards (keys compared by name, tag, break
//! behaviour, adapter and revoked state, since their ids and secrets are random).

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{Site, accounts_file, ctx_of, decode, files_under, normal, refusal, section, twin_of};
use nullrouter_dashboard::pages::Id;
use nullrouter_engine::accounts::SecretSource;
use nullrouter_registry::{OperatorHome, SecretString};
use nullrouter_server::ops::{Ctx, Done, OpError, accounts as acc, keys as kops};
use nullrouter_server::views::{Live, accounts as accounts_view, change_log, keys as keys_view};
use reqwest::StatusCode;
use serde_json::{Value, json};

const PROVIDER: &str = "anthropic";
const BACK_PROVIDER: &str = "/providers/anthropic";
const BACK_KEYS: &str = "/endpoint";

/// One US1 operation, as a form and as the CLI's op.
#[derive(Debug, Clone, Copy)]
enum Step {
    Add {
        name: &'static str,
        secret: &'static str,
        order: Option<&'static str>,
    },
    Priority {
        name: &'static str,
        value: &'static str,
    },
    Disable(&'static str),
    Enable(&'static str),
    Exclusive {
        name: &'static str,
        on: bool,
    },
    Remove(&'static str),
    Issue {
        name: &'static str,
        behaviour: Option<&'static str>,
        harness: Option<&'static str>,
        adapter: Option<&'static str>,
    },
    Tag {
        key: &'static str,
        text: Option<&'static str>,
    },
    SetBreak {
        key: &'static str,
        behaviour: &'static str,
    },
    SetAdapter {
        key: &'static str,
        harness: Option<&'static str>,
    },
    Revoke(&'static str),
}

/// Key names to ids, as one home holds them now.
type Ids = BTreeMap<String, String>;

impl Step {
    fn back(self) -> &'static str {
        match self {
            Step::Issue { .. }
            | Step::Tag { .. }
            | Step::SetBreak { .. }
            | Step::SetAdapter { .. }
            | Step::Revoke(_) => BACK_KEYS,
            _ => BACK_PROVIDER,
        }
    }

    /// The route and the form fields, as the page's form posts them (an absent option is empty).
    fn form(self, ids: &Ids) -> (&'static str, Vec<(&'static str, String)>) {
        let s = |v: Option<&str>| v.unwrap_or("").to_owned();
        let id = |k: &str| ids.get(k).cloned().unwrap_or_else(|| panic!("no key named {k}"));
        match self {
            Step::Add { name, secret, order } => (
                "accounts/add",
                vec![
                    ("provider", PROVIDER.into()),
                    ("name", name.into()),
                    ("secret", secret.into()),
                    ("order", s(order)),
                ],
            ),
            Step::Priority { name, value } => (
                "accounts/priority",
                vec![("provider", PROVIDER.into()), ("name", name.into()), ("priority", value.into())],
            ),
            Step::Disable(name) => ("accounts/disable", vec![("provider", PROVIDER.into()), ("name", name.into())]),
            Step::Enable(name) => ("accounts/enable", vec![("provider", PROVIDER.into()), ("name", name.into())]),
            Step::Exclusive { name, on } => {
                let mut f = vec![("provider", PROVIDER.into()), ("name", name.into())];
                if on {
                    f.push(("on", "on".into()));
                }
                ("accounts/exclusive", f)
            }
            Step::Remove(name) => ("accounts/remove", vec![("provider", PROVIDER.into()), ("name", name.into())]),
            Step::Issue { name, behaviour, harness, adapter } => (
                "keys/issue",
                vec![("name", name.into()), ("break", s(behaviour)), ("harness", s(harness)), ("adapter", s(adapter))],
            ),
            Step::Tag { key, text } => ("keys/tag", vec![("key", id(key)), ("text", s(text))]),
            Step::SetBreak { key, behaviour } => {
                ("keys/set-break", vec![("key", id(key)), ("behaviour", behaviour.into())])
            }
            Step::SetAdapter { key, harness } => ("keys/set-adapter", vec![("key", id(key)), ("harness", s(harness))]),
            Step::Revoke(key) => ("keys/revoke", vec![("key", id(key))]),
        }
    }

    /// The CLI's op on `ctx`, with the same arguments the form carries.
    fn cli(self, ctx: &Ctx, ids: &Ids) -> Result<(), OpError> {
        let id = |k: &str| ids.get(k).cloned().unwrap_or_else(|| panic!("no key named {k}"));
        let r: Result<Done, OpError> = match self {
            Step::Add { name, secret, order } => acc::add(
                ctx,
                PROVIDER,
                name,
                SecretSource::Literal,
                Some(SecretString::new(secret)),
                order.map(|o| o.parse::<i64>().expect("a number")),
            ),
            Step::Priority { name, value } => acc::priority(ctx, PROVIDER, name, value),
            Step::Disable(name) => acc::disable(ctx, PROVIDER, name),
            Step::Enable(name) => acc::enable(ctx, PROVIDER, name),
            Step::Exclusive { name, on } => acc::exclusive(ctx, PROVIDER, name, on),
            Step::Remove(name) => acc::remove(ctx, PROVIDER, name),
            Step::Issue { name, behaviour, harness, adapter } => kops::issue(ctx, name, behaviour, harness, adapter),
            Step::Tag { key, text } => kops::tag(ctx, &id(key), text, text.is_none()),
            Step::SetBreak { key, behaviour } => kops::set_break(ctx, &id(key), behaviour),
            Step::SetAdapter { key, harness } => kops::set_adapter(ctx, &id(key), harness, harness.is_none()),
            Step::Revoke(key) => kops::revoke(ctx, &id(key)),
        };
        r.map(|_| ())
    }
}

/// The steps of US1, in the order an operator would take them.
fn us1_steps() -> Vec<Step> {
    vec![
        Step::Add { name: "work", secret: "sk-work-0000001", order: None },
        Step::Add { name: "side", secret: "sk-side-0000002", order: Some("5") },
        Step::Priority { name: "work", value: "2" },
        Step::Priority { name: "work", value: "not-a-number" },
        Step::Disable("side"),
        Step::Enable("side"),
        Step::Exclusive { name: "work", on: true },
        Step::Exclusive { name: "work", on: false },
        Step::Remove("side"),
        Step::Issue { name: "laptop", behaviour: Some("restart"), harness: Some("claude-code"), adapter: None },
        Step::Issue { name: "ci", behaviour: None, harness: None, adapter: None },
        Step::Tag { key: "ci", text: Some("nightly") },
        Step::Tag { key: "ci", text: None },
        Step::SetBreak { key: "ci", behaviour: "error_event" },
        Step::SetAdapter { key: "laptop", harness: Some("claude-code") },
        Step::SetAdapter { key: "laptop", harness: None },
        Step::Revoke("ci"),
    ]
}

fn key_ids(home: &Path) -> Ids {
    let v = keys_view::build(&OperatorHome::new(home), &json!({}), &Live::none()).expect("the keys view").json;
    v.as_array()
        .unwrap()
        .iter()
        .map(|k| (k["name"].as_str().unwrap().to_owned(), k["id"].as_str().unwrap().to_owned()))
        .collect()
}

/// `accounts list --json`, as the view builds it without a server.
fn accounts_list(home: &Path) -> Value {
    normal(&accounts_view::build(&OperatorHome::new(home), &json!({}), &Live::none()).expect("the accounts view").json)
}

/// `keys list --json` by name: tag, break behaviour, adapter and whether revoked. Ids, digests and
/// the time of issue are random, so they are left out.
fn keys_list(home: &Path) -> Value {
    let v = keys_view::build(&OperatorHome::new(home), &json!({}), &Live::none()).expect("the keys view").json;
    json!(
        v.as_array()
            .unwrap()
            .iter()
            .map(|k| json!({
                "name": k["name"],
                "harness": k["harness"],
                "break": k["break"],
                "adapter": k.get("adapter").cloned().unwrap_or(Value::Null),
                "revoked": !k["revoked"].is_null(),
            }))
            .collect::<Vec<_>>()
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn every_us1_op_through_the_browser_leaves_the_same_state_as_the_cli() {
    let site = Site::start().await;
    let twin = twin_of(site.home());
    for step in us1_steps() {
        let dash_ids = key_ids(site.home());
        let cli_ids = key_ids(twin.path());
        let (route, fields) = step.form(&dash_ids);
        let flash = site.flow(route, step.back(), &fields).await;
        let cli = refusal(step.cli(&ctx_of(twin.path()), &cli_ids));
        assert_eq!(flash.refused, cli, "{step:?}: the page and the CLI disagree");
    }
    assert_eq!(accounts_file(site.home()), accounts_file(twin.path()), "accounts.toml differs");
    assert_eq!(accounts_list(site.home()), accounts_list(twin.path()), "accounts list --json differs");
    assert_eq!(keys_list(site.home()), keys_list(twin.path()), "keys list --json differs");
    // The comparison must not pass on two empty homes: the steps left work behind.
    assert_ne!(accounts_file(site.home()), Value::Null);
    let keys = keys_list(site.home());
    assert_eq!(keys.as_array().map(Vec::len), Some(2), "{keys}");
    assert!(keys.to_string().contains("\"revoked\":true"), "{keys}");
}

#[tokio::test(flavor = "multi_thread")]
async fn binding_an_adapter_that_is_not_installed_goes_through_the_form_as_it_does_through_the_cli() {
    // The CLI does not check for an installed adapter when it binds one; the form must not either.
    let site = Site::start().await;
    let twin = twin_of(site.home());
    let step = Step::Issue { name: "bound", behaviour: None, harness: None, adapter: Some("claude-code") };
    let (route, fields) = step.form(&key_ids(site.home()));
    let dash = site.flow(route, BACK_KEYS, &fields).await;
    let cli = refusal(step.cli(&ctx_of(twin.path()), &key_ids(twin.path())));
    assert_eq!(dash.refused, cli);
    assert_eq!(keys_list(site.home()), keys_list(twin.path()));
    let issued = keys_list(site.home());
    assert_eq!(issued[0]["adapter"], json!("claude-code"), "{issued}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_add_shows_the_cli_refusal_beside_the_name_and_keeps_the_other_fields() {
    let site = Site::start().await;
    let fields = [
        ("provider", PROVIDER.to_owned()),
        ("name", "Bad Name".to_owned()),
        ("secret", "sk-bad-0000004".to_owned()),
        ("order", "7".to_owned()),
    ];
    // The CLI's refusal for the same add, on a twin of the same (empty) home.
    let twin = twin_of(site.home());
    let cli = acc::add(
        &ctx_of(twin.path()),
        PROVIDER,
        "Bad Name",
        SecretSource::Literal,
        Some(SecretString::new("sk-bad-0000004")),
        Some(7),
    )
    .expect_err("the CLI refuses an invalid name");

    let flash = site.flow("accounts/add", BACK_PROVIDER, &fields).await;
    assert_eq!(flash.refused.as_deref(), Some(cli.text.as_str()));

    // The add form keeps the name and the order, names the refusal beside the name, and never
    // shows the secret.
    let form = section(&flash.page, "action=\"/do/accounts/add\"", "</form>");
    assert!(form.contains("name=\"name\" value=\"Bad Name\""), "{form}");
    assert!(form.contains("name=\"order\" value=\"7\""), "{form}");
    let name_field = section(form, "name=\"name\"", "</label>");
    assert!(name_field.contains("field__error") && decode(name_field).contains(&cli.text), "{name_field}");
    assert!(!flash.page.contains("sk-bad-0000004"), "the secret came back on the page");
    assert_eq!(accounts_file(site.home()), Value::Null, "a refused add wrote accounts.toml");
}

#[tokio::test(flavor = "multi_thread")]
async fn adding_an_existing_name_replaces_the_account_as_the_cli_does() {
    // The CLI has no duplicate refusal: `accounts add` on a taken name replaces the account. The
    // form does the same, and both homes end with one account of that name.
    let site = Site::start().await;
    let twin = twin_of(site.home());
    let add = |secret: &'static str| Step::Add { name: "work", secret, order: None };
    let fields = |secret: &str| {
        [
            ("provider", PROVIDER.to_owned()),
            ("name", "work".to_owned()),
            ("secret", secret.to_owned()),
            ("order", String::new()),
        ]
    };
    for secret in ["sk-first-0000005", "sk-second-000006"] {
        let flash = site.flow("accounts/add", BACK_PROVIDER, &fields(secret)).await;
        assert_eq!(flash.refused, None, "{secret}");
        assert_eq!(refusal(add(secret).cli(&ctx_of(twin.path()), &Ids::new())), None, "{secret}");
    }
    assert_eq!(accounts_file(site.home()), accounts_file(twin.path()));
    let text = std::fs::read_to_string(site.home().join(nullrouter_engine::accounts::FILE)).unwrap();
    assert_eq!(text.matches("name = \"work\"").count(), 1, "{text}");
    assert!(text.contains("sk-second-000006") && !text.contains("sk-first-0000005"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_key_shows_once_and_never_after_a_reload() {
    let site = Site::start().await;
    let fields = [
        ("name", "once-key".to_owned()),
        ("break", String::new()),
        ("harness", String::new()),
        ("adapter", String::new()),
    ];
    let flash = site.flow("keys/issue", BACK_KEYS, &fields).await;
    let marker = "once__value\">";
    let at = flash.page.find(marker).expect("the new key is shown");
    let rest = &flash.page[at + marker.len()..];
    let key = decode(&rest[..rest.find("</pre>").unwrap()]);
    assert!(key.len() > 8, "{key:?}");
    assert_eq!(flash.page.matches(&key).count(), 1, "the key is shown once on its page");

    let (_, again) = site.get(&flash.location).await;
    assert!(!again.contains(&key), "a reload shows the key again");
    let (_, later) = site.get(BACK_KEYS).await;
    assert!(!later.contains(&key), "a later page shows the key");

    let keys_file = std::fs::read_to_string(site.home().join(nullrouter_engine::keys::FILE)).unwrap();
    assert!(!keys_file.contains(&key), "the key is stored in keys.toml");
    let log = std::fs::read_to_string(change_log::path(&site.d.home())).unwrap();
    assert!(!log.contains(&key), "the change log holds the key");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pasted_secret_is_in_no_response_and_no_file_but_the_account_store() {
    let site = Site::start().await;
    let mut raw = [0u8; 8];
    getrandom::fill(&mut raw).unwrap();
    let marker = format!("nrmarker-{}", raw.iter().map(|b| format!("{b:02x}")).collect::<String>());

    let flash = site
        .flow(
            "accounts/add",
            BACK_PROVIDER,
            &[
                ("provider", PROVIDER.to_owned()),
                ("name", "marked".to_owned()),
                ("secret", marker.clone()),
                ("order", String::new()),
            ],
        )
        .await;
    assert_eq!(flash.refused, None, "the marked account was refused");

    // Every page, as a crawl sees it, after the change.
    for id in Id::ALL {
        let path = id.path();
        let (status, _) = site.get(path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
    site.get(BACK_PROVIDER).await;

    let bodies = site.seen.lock().unwrap().clone();
    for body in &bodies {
        assert!(!body.contains(&marker), "a response carries the secret");
    }

    let holding: Vec<String> = files_under(site.home())
        .into_iter()
        .filter(|f| std::fs::read(site.home().join(f)).unwrap().windows(marker.len()).any(|w| w == marker.as_bytes()))
        .collect();
    assert_eq!(holding, [nullrouter_engine::accounts::FILE.to_owned()], "the secret is in a file other than the store");
    let log = std::fs::read_to_string(change_log::path(&site.d.home())).unwrap();
    assert!(!log.contains(&marker), "the change log holds the secret");
}
