//! `nullrouter plugins` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;

use std::io::{self, Write};

use clap::{Subcommand, ValueEnum};
use nullrouter_registry::OperatorHome;
use nullrouter_server::ops::plugins;
use nullrouter_server::ops::{Ctx, Status};
use nullrouter_server::views;
use serde_json::json;

#[derive(Subcommand)]
pub(crate) enum Command {
    List {
        /// Include the community set with its fit status.
        #[arg(long)]
        community: bool,
    },
    /// Install a community plugin. Exit 3 if this core can't support it.
    Install {
        id: String,
    },
    Uninstall {
        id: String,
    },
    /// Let plugins reach this machine and the local network, or stop them.
    PrivateEndpoints {
        on: Switch,
        /// Don't ask before turning it on.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Switch {
    On,
    Off,
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let fail = |text: String, code: u8| {
        eprintln!("{text}");
        ExitCode::from(code)
    };
    let home = || home.clone().map_or_else(OperatorHome::resolve, OperatorHome::new);
    let ctx = || Ctx { home: home(), engine: None };
    let mut log = Vec::new();
    let result = match cmd {
        Command::List { community } => return list(home(), community, as_json),
        Command::Install { id } => plugins::install_with(&ctx(), &mut log, &id),
        Command::Uninstall { id } => plugins::uninstall_with(&ctx(), &mut log, &id),
        Command::PrivateEndpoints { on, yes } => {
            let on = on == Switch::On;
            if on && !yes {
                if as_json {
                    return Err(fail("--json needs --yes".into(), 2));
                }
                eprint!("{} Continue? [y/N] ", plugins::private_endpoints_question());
                io::stderr().flush().map_err(|e| fail(e.to_string(), 1))?;
                let mut line = String::new();
                io::stdin().read_line(&mut line).map_err(|e| fail(e.to_string(), 1))?;
                if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    println!("not changed");
                    return Err(ExitCode::from(1));
                }
            }
            plugins::private_endpoints_with(&ctx(), &mut log, on)
        }
    };
    for l in &log {
        eprintln!("{l}");
    }
    let r = result.map_err(|e| fail(e.text, e.exit))?;
    if let Status::SavedNotApplied { reason } = &r.done.status {
        return Err(fail(reason.clone(), 1));
    }
    if as_json {
        println!("{}", r.json);
    } else {
        println!("{}", r.done.text);
    }
    Ok(ExitCode::SUCCESS)
}

/// Bundled and installed plugins as loaded, then, with `--community`, every community
/// plugin with its fit status.
fn list(home: OperatorHome, with_community: bool, as_json: bool) -> Result<ExitCode, ExitCode> {
    let view = super::read(&home, views::plugins::NEEDS, &json!({"community": with_community}), views::plugins::build)?;
    if as_json {
        println!("{:#}", view.json);
    } else {
        for r in view.json.as_array().into_iter().flatten() {
            let s = |k: &str| r[k].as_str().unwrap_or_default().to_owned();
            println!("{:<24} {:<10} {}", s("id"), s("set"), s("status"));
        }
    }
    Ok(ExitCode::SUCCESS)
}
