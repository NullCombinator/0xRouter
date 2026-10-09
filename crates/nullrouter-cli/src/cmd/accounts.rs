//! `nullrouter accounts` (contracts/operator-cli.md).

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::SystemTime;

use clap::Subcommand;
use nullrouter_cli::signin::{self, SignIn};
use nullrouter_engine::accounts::{self, Account, Accounts, SecretSource};
use nullrouter_engine::clock;
use nullrouter_engine::signin::SignInHttp;
use nullrouter_engine::tokens;
use nullrouter_registry::{OperatorHome, SecretString};
use nullrouter_server::views::{self, View};
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
    /// Set the priority: how much cold work the account takes, 0 for none.
    Priority {
        provider: String,
        name: String,
        /// A number of 0 or more; 1 is the default.
        #[arg(allow_hyphen_values = true)]
        priority: String,
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
    /// Declare an account in exclusive use (FR-023): used only through 0router. `on` keeps an
    /// earlier declaration's start.
    Exclusive {
        provider: String,
        name: String,
        /// `on` or `off`.
        #[arg(value_parser = ["on", "off"])]
        state: String,
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
    if let Command::List { provider, long } = &cmd {
        let args = json!({ "provider": provider });
        let view = super::read(&home, views::accounts::NEEDS, &args, views::accounts::build)?;
        let list = Accounts::load(&home.path().join(accounts::FILE)).map_err(fail)?;
        print_list(&view, &list, *long, as_json);
        return Ok(ExitCode::SUCCESS);
    }
    let mut list = Accounts::load(&home.path().join(accounts::FILE)).map_err(fail)?;
    // What the command did beyond `accounts.toml`, for its line.
    let mut note = None;
    let mut removed = false;
    let (provider, name) = match cmd {
        Command::List { .. } => unreachable!("handled above"),
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
        Command::Priority { provider, name, priority } => {
            let n = priority
                .parse::<f64>()
                .map_err(|_| fail(format!("{priority:?}: priority must be a number of 0 or more")))?;
            list.set_priority(&provider, &name, n).map_err(fail)?;
            (provider, name)
        }
        Command::Remove { provider, name } => {
            list.remove(&provider, &name).map_err(fail)?;
            removed = true;
            // Its tokens go too, under the writers' lock; its quota history stays
            // (`quota forget` deletes it).
            if tokens::remove(home.path(), &provider, &name).map_err(fail)? {
                note = Some("removed with its tokens");
            }
            (provider, name)
        }
        Command::Disable { provider, name } => {
            list.set_disabled(&provider, &name, true).map_err(fail)?;
            (provider, name)
        }
        Command::Enable { provider, name } => {
            list.set_disabled(&provider, &name, false).map_err(fail)?;
            // A refused account is tried again (research R10).
            if tokens::clear_refused(home.path(), &provider, &name).map_err(fail)? {
                note = Some("no longer marked refused");
            }
            (provider, name)
        }
        Command::Exclusive { provider, name, state } => {
            let Some(current) = list.get(&provider, &name) else {
                return Err(fail(accounts::AccountError::NotFound { provider, name }));
            };
            let since = if state == "on" {
                let reg = crate::open(Some(home.path().to_owned()))?.snapshot();
                let entity = reg.provider(&provider).map_err(fail)?;
                if let Some(problem) = current.exclusive_use_problem(entity) {
                    eprintln!("{problem}");
                    return Err(ExitCode::from(2));
                }
                Some(current.exclusive_use.unwrap_or_else(clock::now))
            } else {
                None
            };
            list.set_exclusive_use(&provider, &name, since).map_err(fail)?;
            (provider, name)
        }
    };
    list.save().map_err(fail)?;
    let status = super::apply(&home).map_err(fail)?;
    // A running server drops the account's fingerprints and ledger entries when it reloads; with
    // none running, the warm file is edited here.
    if removed
        && status != "applied"
        && let Err(e) = nullrouter_engine::journal::state::forget_account(home.path(), &format!("{provider}/{name}"))
    {
        eprintln!("note: the routing state of {provider}/{name} could not be cleaned: {e}");
    }
    if as_json {
        println!("{}", json!({"provider": provider, "name": name, "note": note, "status": status}));
    } else if let Some(note) = note {
        println!("{provider}/{name}: {note}; {status}");
    } else {
        println!("{provider}/{name}: {status}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Columns padded to their widest cell (header included), two spaces apart; the last
/// column isn't padded.
pub(crate) fn table(rows: &[Vec<String>]) -> Vec<String> {
    let cols = rows.first().map_or(0, Vec::len);
    let widths: Vec<usize> = (0..cols).map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0)).collect();
    rows.iter()
        .map(|r| {
            let mut line = String::new();
            for (i, cell) in r.iter().enumerate() {
                if i + 1 == cols {
                    line.push_str(cell);
                } else {
                    line.push_str(cell);
                    line.push_str(&" ".repeat(widths[i] - cell.chars().count() + 2));
                }
            }
            line
        })
        .collect()
}

/// The declared exclusive-use time of a listed account (FR-023), or `None`.
fn exclusive_of(list: &Accounts, r: &Value) -> Option<SystemTime> {
    list.get(r["provider"].as_str()?, r["name"].as_str()?)?.exclusive_use
}

fn print_list(view: &View, list: &Accounts, long: bool, as_json: bool) {
    // A token file that doesn't load is a warning, in either output.
    for w in view.extra["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        eprintln!("{w}");
    }
    let rows: &[Value] = view.json.as_array().map_or(&[], Vec::as_slice);
    if as_json {
        let mut json = view.json.clone();
        for r in json.as_array_mut().into_iter().flatten() {
            r["exclusive_use"] = exclusive_of(list, r).map_or(Value::Null, |t| json!(clock::rfc3339(t)));
        }
        println!("{:#}", json);
        return;
    }
    let text = |r: &Value, k: &str| r[k].as_str().unwrap_or("-").to_owned();
    // `since <date>` (UTC, YYYY-MM-DD) or blank.
    let since = |r: &Value| {
        exclusive_of(list, r).map_or_else(String::new, |t| {
            format!("since {}", clock::rfc3339(t).split('T').next().unwrap_or(""))
        })
    };
    let mut cells = vec![
        ["provider", "name", "kind", "order", "priority", "secret", "proxy", "exclusive", "state"]
            .map(str::to_owned)
            .to_vec(),
    ];
    cells.extend(rows.iter().map(|r| {
        vec![
            text(r, "provider"),
            text(r, "name"),
            text(r, "kind"),
            r["order"].to_string(),
            r["priority"].as_f64().map_or_else(|| "-".into(), |p| p.to_string()),
            text(r, "secret"),
            match (r["proxy"].as_str(), r["proxy_level"].as_str()) {
                (Some(name), Some(level)) => format!("{name} ({level})"),
                _ => "—".to_owned(),
            },
            since(r),
            text(r, "state_text"),
        ]
    }));
    for (line, r) in table(&cells).iter().zip(std::iter::once(None).chain(rows.iter().map(Some))) {
        println!("{line}");
        if let Some(r) = r.filter(|r| long && r["kind"] == "signin") {
            println!(
                "    email {}  tier {}  expires {}  refreshed {}",
                text(r, "email"),
                text(r, "tier"),
                text(r, "expires_at"),
                text(r, "last_refresh_at")
            );
        }
    }
    // The accounts the operator has to sign in again.
    for r in rows.iter().filter(|r| matches!(r["state"].as_str(), Some("needs_sign_in" | "refused"))) {
        println!("  → run: nullrouter accounts signin {} {}", text(r, "provider"), text(r, "name"));
    }
}
