//! `nullrouter accounts` (contracts/operator-cli.md).

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_cli::signin::{self, SignIn};
use nullrouter_engine::accounts::{self, Account, Accounts, SecretSource};
use nullrouter_engine::clock::rfc3339;
use nullrouter_engine::signin::SignInHttp;
use nullrouter_engine::tokens::TokenStore;
use nullrouter_registry::{OperatorHome, SecretString};
use nullrouter_server::operator;
use serde_json::{Value, json};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Add an account. The secret is read from stdin unless `--env` names a variable.
    Add {
        provider: String,
        name: String,
        #[arg(long, value_name = "VAR")]
        env: Option<String>,
        #[arg(long, value_name = "N")]
        order: Option<i64>,
    },
    /// Sign in to a provider's account in the browser (or on another device) and add it.
    Signin {
        provider: String,
        name: String,
        /// Don't open a browser; paste the address or code.
        #[arg(long)]
        paste: bool,
        /// Don't open a browser; only print the link.
        #[arg(long)]
        no_browser: bool,
        /// Answer the terms question with yes (the warning is still printed).
        #[arg(long)]
        accept_terms_risk: bool,
    },
    /// List accounts, optionally for one provider.
    List {
        provider: Option<String>,
        /// Also show sign-in email, tier, token expiry and last refresh.
        #[arg(long)]
        long: bool,
    },
    Remove {
        provider: String,
        name: String,
    },
    Disable {
        provider: String,
        name: String,
    },
    Enable {
        provider: String,
        name: String,
    },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

/// The secret piped in on stdin, without its trailing newline. A terminal works too, but
/// what is typed shows on screen, so say so first.
fn secret_from_stdin() -> Result<String, ExitCode> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprintln!("warning: reading the secret from the terminal; it is not hidden as you type.");
        eprintln!("Pipe it in instead, for example: printenv MY_KEY | nullrouter accounts add …");
        eprintln!("Type the secret, then Enter and Ctrl-D:");
    }
    let mut s = String::new();
    stdin.lock().read_to_string(&mut s).map_err(fail)?;
    let s = s.trim_end_matches(['\r', '\n']).to_owned();
    if s.trim().is_empty() {
        return Err(fail("no secret on stdin"));
    }
    Ok(s)
}

/// `accounts signin`: resolves the provider, reads stdin lines on a thread, and turns
/// Ctrl-C into cancellation (exit 5, nothing written).
fn sign_in(
    home: &OperatorHome,
    provider: &str,
    name: &str,
    browser: bool,
    accept_terms_risk: bool,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    if !accounts::valid_name(name) {
        return Err(fail(accounts::AccountError::BadName(name.into())));
    }
    let reg = crate::open(Some(home.path().to_owned()))?.snapshot();
    let entity = reg.provider(provider).map_err(fail)?;
    let http = SignInHttp::new(reg.runtime().allow_private_endpoints);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(fail)?;

    let (tx, mut input) = tokio::sync::mpsc::unbounded_channel();
    // A plain thread: a blocking stdin read must not hold the runtime open at exit.
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            if line.ok().is_none_or(|l| tx.send(l).is_err()) {
                break;
            }
        }
    });
    let open: &dyn Fn(&str) = &signin::open_browser;
    let job = SignIn { home, provider: entity, name, http: &http, accept_terms_risk, browser: browser.then_some(open) };
    // With --json, stdout carries only the result.
    let mut out: Box<dyn std::io::Write> =
        if as_json { Box::new(std::io::stderr()) } else { Box::new(std::io::stdout()) };
    let result = rt.block_on(async {
        let cancel = tokio_util::sync::CancellationToken::new();
        let on_ctrl_c = cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                on_ctrl_c.cancel();
            }
        });
        job.run(&mut input, &mut out, &cancel).await
    });
    match result {
        Ok(done) if as_json => {
            println!(
                "{}",
                json!({
                    "provider": done.provider,
                    "name": done.name,
                    "email": done.email,
                    "tier": done.tier,
                    "replaced": done.replaced.is_some(),
                    "status": done.status,
                })
            );
            Ok(ExitCode::SUCCESS)
        }
        Ok(done) => {
            println!("{}", done.line());
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            eprintln!("{}", e.message);
            Err(ExitCode::from(e.code))
        }
    }
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let mut list = Accounts::load(&home.path().join(accounts::FILE)).map_err(fail)?;
    let (provider, name) = match cmd {
        Command::List { provider, long } => {
            return Ok(print_list(&home, &list, provider.as_deref(), long, as_json));
        }
        Command::Signin { provider, name, paste, no_browser, accept_terms_risk } => {
            let browser = !paste && !no_browser && signin::has_display();
            return sign_in(&home, &provider, &name, browser, accept_terms_risk, as_json);
        }
        Command::Add { provider, name, env, order } => {
            if !accounts::valid_name(&name) {
                return Err(fail(accounts::AccountError::BadName(name)));
            }
            // The secret may only go to the hosts the provider sends to now (FR-044).
            let reg = crate::open(Some(home.path().to_owned()))?.snapshot();
            let hosts = match reg.provider(&provider) {
                Ok(p) => accounts::provider_hosts(p),
                Err(e) => return Err(fail(e)),
            };
            let (source, secret) = match env {
                Some(var) => {
                    let secret = std::env::var(&var).ok().filter(|s| !s.is_empty()).map(SecretString::new);
                    if secret.is_none() {
                        eprintln!("warning: {var} is not set here; the server reads it when it starts");
                    }
                    (SecretSource::Env(var), secret)
                }
                None => (SecretSource::Literal, Some(SecretString::new(secret_from_stdin()?))),
            };
            // Without `--order`, a new account goes after the provider's others.
            let order = order
                .unwrap_or_else(|| list.iter().filter(|a| a.provider == provider).map(|a| a.order).max().unwrap_or(0));
            let account = Account::key(provider.clone(), name.clone(), source, secret, order, hosts);
            list.add(account).map_err(fail)?;
            (provider, name)
        }
        Command::Remove { provider, name } => {
            list.remove(&provider, &name).map_err(fail)?;
            (provider, name)
        }
        Command::Disable { provider, name } => {
            list.set_disabled(&provider, &name, true).map_err(fail)?;
            (provider, name)
        }
        Command::Enable { provider, name } => {
            list.set_disabled(&provider, &name, false).map_err(fail)?;
            (provider, name)
        }
    };
    list.save().map_err(fail)?;
    let status = super::apply(&home).map_err(fail)?;
    if as_json {
        println!("{}", json!({"provider": provider, "name": name, "status": status}));
    } else {
        println!("{provider}/{name}: {status}");
    }
    Ok(ExitCode::SUCCESS)
}

/// `active`, `disabled`, or the running server's rests: `cooling <model> 12 s, …`.
fn state(a: &Account, live: &Value) -> String {
    if a.disabled {
        return "disabled".into();
    }
    let rests: Vec<String> = live["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["provider"] == a.provider.as_str() && s["name"] == a.name.as_str())
        .flat_map(|s| s["cooling"].as_array().cloned().unwrap_or_default())
        .map(|c| {
            format!(
                "{} {} s",
                c["model"].as_str().unwrap_or("?"),
                c["remaining_ms"].as_u64().unwrap_or(0).div_ceil(1000)
            )
        })
        .collect();
    if rests.is_empty() { "active".into() } else { format!("cooling {}", rests.join(", ")) }
}

fn print_list(home: &OperatorHome, list: &Accounts, provider: Option<&str>, long: bool, as_json: bool) -> ExitCode {
    // Cooldowns live in the running server; without one, every account is at rest.
    let live = operator::call(home, &json!({"op": "accounts.state"})).unwrap_or(Value::Null);
    // Sign-in accounts show their access token's last four, as keys do (research R5).
    let tokens = TokenStore::load(home.path()).unwrap_or_else(|e| {
        eprintln!("warning: {e}");
        TokenStore::default()
    });
    let rows: Vec<_> = list
        .iter()
        .filter(|a| provider.is_none_or(|p| a.provider == p))
        .map(|a| {
            let t = tokens.get(&a.provider, &a.name).filter(|_| a.is_signin());
            let secret = match t {
                Some(t) => t.shown_token(),
                None if a.is_signin() => "…".to_owned(),
                None => a.shown_secret(),
            };
            json!({
                "provider": a.provider,
                "name": a.name,
                "kind": if a.is_signin() { "signin" } else { "key" },
                "order": a.order,
                "secret": secret,
                "state": state(a, &live),
                "email": t.and_then(|t| t.claims.email.clone()),
                "tier": t.and_then(|t| t.claims.tier.clone()),
                "expires_at": t.map(|t| rfc3339(t.expires_at)),
                "last_refresh_at": t.and_then(|t| t.last_refresh_at.map(rfc3339)),
            })
        })
        .collect();
    if as_json {
        println!("{:#}", json!(rows));
    } else {
        for r in &rows {
            let text = |k: &str| r[k].as_str().unwrap_or("-").to_owned();
            let mut line = format!(
                "{:<20} {:<20} {:<6} {:>5} {:<24} {}",
                text("provider"),
                text("name"),
                text("kind"),
                r["order"],
                text("secret"),
                text("state")
            );
            if long && r["kind"] == "signin" {
                line = format!(
                    "{line}\n    email {}  tier {}  expires {}  refreshed {}",
                    text("email"),
                    text("tier"),
                    text("expires_at"),
                    text("last_refresh_at")
                );
            }
            println!("{line}");
        }
    }
    ExitCode::SUCCESS
}
