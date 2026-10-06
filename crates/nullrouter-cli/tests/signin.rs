//! `accounts signin` (spec 005 T025; contracts/operator-cli.md § `accounts signin` output;
//! research R3, R4; Clarifications Q2).
//!
//! The binary signs in only to the providers in the operator's registry, whose bundled
//! sign-in URLs point at the real identity providers, and a user plugin can't declare
//! `[signin]` (the fit check keeps it to bundled plugins). So flows that reach an identity
//! provider run the library driver the binary calls, `nullrouter_cli::signin::SignIn`, with
//! providers gated as bundled and pointed at `MockIdp`. What needs no network (the terms
//! refusal, input errors) runs through the binary itself.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nullrouter_cli::signin::{Done, EXIT_ENDED, PASTE_PROMPT, SignIn, TERMS_WARNING};
use nullrouter_engine::accounts::{self, AccountKind, Accounts, SecretSource};
use nullrouter_engine::signin::SignInHttp;
use nullrouter_engine::testkit::mock_idp::{AUTHORIZE, CLIENT_ID, DEVICE, DISCOVERY, PROFILE, TOKEN};
use nullrouter_engine::testkit::{DevicePoll, MockIdp};
use nullrouter_engine::tokens::{self, TokenStore};
use nullrouter_registry::validate::{GateCtx, validate_with};
use nullrouter_registry::{OperatorHome, PluginSource, ProviderEntity, SecretString};
use serde_json::json;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;

// ---- the binary --------------------------------------------------------------------------

fn nr(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nullrouter"))
        .arg("--home")
        .arg(home)
        .args(args)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A command that exits without reading stdin closes the pipe first; that is not a failure.
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    child.wait_with_output().unwrap()
}

fn nothing_written(home: &Path) {
    assert!(!home.join(accounts::FILE).exists(), "no account written");
    assert!(!home.join(tokens::FILE).exists(), "no tokens written");
}

#[test]
fn anthropic_terms_refused_exits_5_with_nothing_written() {
    for answer in ["n\n", "no\n", "\n", ""] {
        let dir = tempfile::tempdir().unwrap();
        let out = nr(dir.path(), &["accounts", "signin", "anthropic", "max"], answer);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(5), "{answer:?}: {stdout}{stderr}");
        assert!(stdout.starts_with(TERMS_WARNING), "the warning comes first: {stdout}");
        assert!(!stdout.contains("http"), "no link before the terms are accepted: {stdout}");
        assert_eq!(stderr.trim(), "anthropic/max: sign-in ended: the terms risk was not accepted; nothing saved");
        nothing_written(dir.path());
    }
}

#[test]
fn bad_input_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    for (args, why) in [
        (["accounts", "signin", "no-such-provider", "x"], "unknown provider"),
        (["accounts", "signin", "anthropic", "Bad Name"], "bad name"),
        (["accounts", "signin", "opencode-go", "main"], "no [signin]"),
    ] {
        let out = nr(dir.path(), &args, "y\n");
        assert_eq!(out.status.code(), Some(1), "{why}: {}", String::from_utf8_lossy(&out.stderr));
        nothing_written(dir.path());
    }
    let out = nr(dir.path(), &["accounts", "signin", "opencode-go", "main"], "");
    assert!(String::from_utf8_lossy(&out.stderr).contains("accounts add opencode-go main"));
}

// ---- the driver against the mock identity provider ---------------------------------------

fn provider(idp: &MockIdp, id: &str, signin: &str) -> ProviderEntity {
    let src = format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"oauth\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n{signin}",
        idp.url("/v1/chat/completions")
    );
    let ctx = GateCtx { allow_private: true, ..GateCtx::default() };
    validate_with(&src, PluginSource::Bundled, &format!("{id}.toml"), &ctx).unwrap_or_else(|e| panic!("{e:#?}")).entity
}

fn grok_cli(idp: &MockIdp) -> ProviderEntity {
    provider(
        idp,
        "grok-cli",
        &format!(
            "[signin]\nflow = \"device_code\"\nclient_id = \"{CLIENT_ID}\"\nscopes = [\"openid\", \"offline_access\"]\ndevice_url = \"{}\"\ntoken_url = \"{}\"\nparams = {{ referrer = \"grok-build\" }}\nrefresh_lead = \"5m\"\n",
            idp.url(DEVICE),
            idp.url(TOKEN)
        ),
    )
}

fn xai(idp: &MockIdp, port: u16) -> ProviderEntity {
    provider(
        idp,
        "xai",
        &format!(
            "[signin]\nflow = \"pkce\"\nclient_id = \"{CLIENT_ID}\"\nscopes = [\"openid\", \"email\", \"offline_access\"]\ndiscovery_url = \"{}\"\nauthorize_url = \"{}\"\ntoken_url = \"{}\"\nredirect = [{{ uri = \"http://127.0.0.1:{port}/callback\", kind = \"loopback\" }}]\nverifier_bytes = 96\nrefresh_lead = \"5m\"\n[signin.profile]\nurl = \"{}\"\nemail = \"email\"\nuser_id = \"userId\"\ntier = \"subscriptionTier\"\n",
            idp.url(DISCOVERY),
            idp.url(AUTHORIZE),
            idp.url(TOKEN),
            idp.url(PROFILE)
        ),
    )
}

fn anthropic(idp: &MockIdp) -> ProviderEntity {
    provider(
        idp,
        "anthropic",
        &format!(
            "[signin]\nflow = \"pkce\"\nclient_id = \"{CLIENT_ID}\"\nscopes = [\"user:inference\"]\nauthorize_url = \"{}\"\ntoken_url = \"{}\"\nredirect = [{{ uri = \"https://console.example.com/oauth/code/callback\", kind = \"code_page\" }}]\nparams = {{ code = \"true\" }}\nbody = \"json\"\nrefresh_lead = \"4h\"\nterms_warning = true\n",
            idp.url(AUTHORIZE),
            idp.url(TOKEN)
        ),
    )
}

/// What the driver printed, readable while it runs.
#[derive(Clone, Default)]
struct Screen(Arc<Mutex<Vec<u8>>>);

impl Write for Screen {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Screen {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    /// Waits for the printed authorize link and the paste prompt after it.
    async fn link(&self) -> String {
        for _ in 0..500 {
            let t = self.text();
            if t.ends_with(PASTE_PROMPT)
                && let Some(l) = t.lines().find(|l| l.starts_with("  http"))
            {
                return l.trim().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no link printed: {}", self.text());
    }
}

fn query(url: &str, key: &str) -> String {
    url::Url::parse(url).unwrap().query_pairs().find(|(k, _)| k == key).unwrap().1.into_owned()
}

/// Runs one sign-in; `script` plays the operator (and the browser) beside it.
async fn sign_in<F: Future<Output = ()>>(
    home: &Path,
    p: &ProviderEntity,
    name: &str,
    accept: bool,
    script: impl FnOnce(Screen, UnboundedSender<String>) -> F,
) -> (Result<Done, nullrouter_cli::signin::Ended>, String) {
    let home = OperatorHome::new(home);
    let http = SignInHttp::new(true).with_timeout(Duration::from_secs(5));
    let job = SignIn { home: &home, provider: p, name, http: &http, accept_terms_risk: accept, browser: None };
    let (tx, mut rx) = unbounded_channel();
    let screen = Screen::default();
    let mut out = screen.clone();
    let cancel = CancellationToken::new();
    let (r, ()) = tokio::join!(job.run(&mut rx, &mut out, &cancel), script(screen.clone(), tx));
    (r, screen.text())
}

fn no_token_shown(idp: &MockIdp, printed: &str) {
    for t in idp.issued() {
        assert!(!printed.contains(&t), "a token was printed");
    }
}

#[tokio::test]
async fn grok_cli_prints_link_code_and_expiry() {
    let idp = MockIdp::start().await;
    idp.set_device_timing(1, 900);
    idp.set_id_claims(Some(json!({ "email": "alice@example.com", "sub": "s-1" })));
    idp.push_device([DevicePoll::Pending]);
    let dir = tempfile::tempdir().unwrap();
    let (r, printed) = sign_in(dir.path(), &grok_cli(&idp), "work", false, |_, _| async {}).await;
    let done = r.unwrap();
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines[0], "Open this page on any device and approve:");
    assert!(lines[1].starts_with("  http") && lines[1].contains("user_code="), "{printed}");
    assert!(lines[2].starts_with("Code: ABCD-") && lines[2].ends_with(" (expires in 15 min)"), "{printed}");
    assert_eq!(lines[3], "Waiting for approval… done.");
    assert_eq!(done.line(), "grok-cli/work: signed in as a…@example.com; saved; applies at next start");
    eprintln!("{printed}{}", done.line());

    let list = Accounts::load(&dir.path().join(accounts::FILE)).unwrap();
    let a = list.get("grok-cli", "work").unwrap();
    assert_eq!(a.kind, AccountKind::Signin);
    assert!(a.secret.is_none() && a.hosts.is_empty(), "a sign-in account carries no secret");
    let store = TokenStore::load(dir.path()).unwrap();
    let e = store.get("grok-cli", "work").unwrap();
    assert!(e.access_token.matches(idp.issued().last().unwrap()));
    no_token_shown(&idp, &printed);
}

#[tokio::test]
async fn xai_reads_the_pasted_address_from_stdin() {
    let idp = MockIdp::start().await;
    // The redirect port is taken, so only the paste can complete the sign-in.
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = xai(&idp, busy.local_addr().unwrap().port());
    let dir = tempfile::tempdir().unwrap();
    let browser = &idp;
    let (r, printed) = sign_in(dir.path(), &p, "main", false, |screen, tx| async move {
        let link = screen.link().await;
        let location = browser.browse(&link).await;
        tx.send("not an address".into()).unwrap();
        while !screen.text().contains("Try again.") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tx.send(format!("  {location}  ")).unwrap();
    })
    .await;
    let done = r.unwrap_or_else(|e| panic!("{e:?}\n{printed}"));
    assert!(printed.starts_with("Open this link in a browser (on any device):\n  http"), "{printed}");
    assert!(
        printed.contains("If the browser ends on a page that doesn't load, copy its full address and paste it here.\n")
    );
    assert!(printed.contains("(No local listener: "), "the busy port is named: {printed}");
    assert_eq!(printed.matches(PASTE_PROMPT).count(), 2, "a refused paste asks again: {printed}");
    assert_eq!(done.line(), "xai/main: signed in as u…@example.com (SuperGrok); saved; applies at next start");
    eprintln!("{printed}\n{}", done.line());
    assert!(TokenStore::load(dir.path()).unwrap().get("xai", "main").is_some());
    no_token_shown(&idp, &printed);
}

/// The operator answers `answer`, then pastes `code#state` from the code page.
async fn approve(idp: &MockIdp, answer: Option<&str>, screen: Screen, tx: UnboundedSender<String>) {
    if let Some(a) = answer {
        tx.send(a.into()).unwrap();
    }
    let link = screen.link().await;
    let location = idp.browse(&link).await;
    tx.send(format!("{}#{}", query(&location, "code"), query(&location, "state"))).unwrap();
}

#[tokio::test]
async fn anthropic_warns_at_every_sign_in_and_needs_yes() {
    let idp = MockIdp::start().await;
    let p = anthropic(&idp);
    let dir = tempfile::tempdir().unwrap();

    // "n": nothing written.
    let (r, printed) = sign_in(dir.path(), &p, "max", false, |_, tx| async move {
        tx.send("n".into()).unwrap();
    })
    .await;
    let e = r.unwrap_err();
    assert_eq!(e.code, EXIT_ENDED);
    assert_eq!(e.message, "anthropic/max: sign-in ended: the terms risk was not accepted; nothing saved");
    assert_eq!(printed, TERMS_WARNING, "nothing after the question");
    nothing_written(dir.path());
    assert_eq!(idp.token_calls(), 0);

    // "y": the warning, then the link.
    let (r, printed) = sign_in(dir.path(), &p, "max", false, |s, tx| approve(&idp, Some("y"), s, tx)).await;
    let done = r.unwrap_or_else(|e| panic!("{e:?}\n{printed}"));
    assert!(
        printed.starts_with(&format!("{TERMS_WARNING}Open this link in a browser (on any device):\n")),
        "{printed}"
    );
    assert!(printed.contains("the page shows a code; paste it here."), "{printed}");
    assert_eq!(done.line(), "anthropic/max: signed in as account; saved; applies at next start");
    eprintln!("{printed}\n{}", done.line());
    let first =
        TokenStore::load(dir.path()).unwrap().get("anthropic", "max").unwrap().access_token.with_exposed(str::to_owned);

    // The same account again: warned again, and the replacement is said.
    let (r, printed) = sign_in(dir.path(), &p, "max", false, |s, tx| approve(&idp, Some("yes"), s, tx)).await;
    assert!(printed.starts_with(TERMS_WARNING), "warned at every sign-in: {printed}");
    let done = r.unwrap();
    assert_eq!(done.replaced, Some(AccountKind::Signin));
    assert!(done.line().contains("; replaced the earlier sign-in; "), "{}", done.line());
    let store = TokenStore::load(dir.path()).unwrap();
    assert!(!store.get("anthropic", "max").unwrap().access_token.matches(&first), "new tokens");
    assert_eq!(store.entries.len(), 1);

    // --accept-terms-risk answers the question but still prints the warning.
    let (r, printed) = sign_in(dir.path(), &p, "max", true, |s, tx| approve(&idp, None, s, tx)).await;
    assert!(printed.starts_with(&format!("{TERMS_WARNING}y (--accept-terms-risk)\n")), "{printed}");
    r.unwrap();
    no_token_shown(&idp, &printed);
}

#[tokio::test]
async fn replacing_a_key_account_says_so_and_keeps_its_order() {
    let idp = MockIdp::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut list = Accounts::load(&dir.path().join(accounts::FILE)).unwrap();
    for (name, order) in [("other", 0), ("work", 3)] {
        let secret = Some(SecretString::new("sk-SENTINEL-0005"));
        let a = accounts::Account::key("grok-cli", name, SecretSource::Literal, secret, order, Default::default());
        list.add(a).unwrap();
    }
    list.save().unwrap();
    let (r, _) = sign_in(dir.path(), &grok_cli(&idp), "work", false, |_, _| async {}).await;
    let done = r.unwrap();
    assert!(done.line().contains("; replaced the key account; "), "{}", done.line());
    let list = Accounts::load(&dir.path().join(accounts::FILE)).unwrap();
    let a = list.get("grok-cli", "work").unwrap();
    assert_eq!((a.kind, a.order), (AccountKind::Signin, 3));
    assert!(a.secret.is_none());
    assert_eq!(list.get("grok-cli", "other").unwrap().kind, AccountKind::Key);
}

#[tokio::test]
async fn refused_and_cancelled_sign_ins_write_nothing() {
    let idp = MockIdp::start().await;
    idp.push_device([DevicePoll::Denied]);
    let dir = tempfile::tempdir().unwrap();
    let (r, _) = sign_in(dir.path(), &grok_cli(&idp), "work", false, |_, _| async {}).await;
    let e = r.unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (EXIT_ENDED, "grok-cli/work: sign-in ended: access_denied; nothing saved")
    );
    nothing_written(dir.path());

    // Input ends (Ctrl-D) with no listener: abandoned.
    let (r, _) = sign_in(dir.path(), &anthropic(&idp), "max", true, |screen, tx| async move {
        screen.link().await;
        drop(tx);
    })
    .await;
    assert_eq!(r.unwrap_err().code, EXIT_ENDED);
    nothing_written(dir.path());

    // Ctrl-C while waiting for the paste.
    let home = OperatorHome::new(dir.path());
    let http = SignInHttp::new(true);
    let p = anthropic(&idp);
    let job = SignIn { home: &home, provider: &p, name: "max", http: &http, accept_terms_risk: true, browser: None };
    let (_tx, mut rx) = unbounded_channel();
    let cancel = CancellationToken::new();
    let screen = Screen::default();
    let mut out = screen.clone();
    let (r, ()) = tokio::join!(job.run(&mut rx, &mut out, &cancel), async {
        screen.link().await;
        cancel.cancel();
    });
    let e = r.unwrap_err();
    assert_eq!((e.code, e.message.as_str()), (EXIT_ENDED, "anthropic/max: sign-in ended: cancelled; nothing saved"));
    nothing_written(dir.path());
}

/// Security review L2: a device link that isn't https (the mock's is http) is printed but
/// never handed to the browser.
#[tokio::test]
async fn only_https_links_reach_the_browser() {
    let idp = MockIdp::start().await;
    let dir = tempfile::tempdir().unwrap();
    let home = OperatorHome::new(dir.path());
    let http = SignInHttp::new(true).with_timeout(Duration::from_secs(5));
    let p = grok_cli(&idp);
    let opened = Mutex::new(Vec::<String>::new());
    let browser = |l: &str| opened.lock().unwrap().push(l.to_owned());
    let job = SignIn {
        home: &home,
        provider: &p,
        name: "work",
        http: &http,
        accept_terms_risk: false,
        browser: Some(&browser),
    };
    let (_tx, mut rx) = unbounded_channel();
    let screen = Screen::default();
    let mut out = screen.clone();
    job.run(&mut rx, &mut out, &CancellationToken::new()).await.unwrap();
    assert!(opened.lock().unwrap().is_empty(), "an http link was opened");
    let printed = screen.text();
    assert!(
        printed.contains("  http://") && printed.contains("(Not opened in the browser: not an https link.)"),
        "{printed}"
    );
}
