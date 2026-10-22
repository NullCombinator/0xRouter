//! `nullrouter routing` (slice 006 contracts/operator-cli.md): the routing view, and the commands
//! that set per-account routing overrides and the amortization window.
//!
//! The view reads the running server over the operator socket (exit 4 with no server). The
//! `set`, `unset` and `window` subcommands edit `accounts.toml` and `config.toml` directly,
//! check the result with the registry gate's rules, and ask a running server to reload.
//! `set-plugin` and `unset-plugin` edit a plugin's `[provider.<id>.meter."<window>"]` tables in
//! `config.toml`: weights and multipliers only, since capacity is per account.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args as ClapArgs, Subcommand};
use nullrouter_cli::routing_text;
use nullrouter_registry::OperatorHome;
use nullrouter_server::ops::accounts::Report;
use nullrouter_server::ops::{self, Ctx, OpError, Status};
use nullrouter_server::views;
use serde_json::json;

/// `routing [target]` shows the routing view; the subcommands change the settings.
#[derive(ClapArgs)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Only this unified model or `provider/model` target.
    target: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Set an account's routing overrides: `cache_lifetime=1h`, `reserve=10%`, `price.input=3`,
    /// `window.<name>.capacity=12000000` (also `.length`, `.reserve`), `window.<name>.weight.<class>=V`
    /// or `window.<name>.multiplier.<glob>=V`.
    Set {
        provider: String,
        name: String,
        #[arg(required = true, value_name = "KEY=VALUE")]
        settings: Vec<String>,
    },
    /// Remove overrides: `cache_lifetime`, `reserve`, `price`, `window.<name>` or one of its fields,
    /// `window.<name>.weight.<class>` or `window.<name>.multiplier.<glob>`.
    Unset {
        provider: String,
        name: String,
        #[arg(required = true, value_name = "KEY")]
        keys: Vec<String>,
    },
    /// Set a plugin's weights and multipliers for every account of it: `window.<name>.weight.<class>=V`
    /// or `window.<name>.multiplier.<glob>=V`. Capacity is per account and is refused here.
    SetPlugin {
        provider: String,
        #[arg(required = true, value_name = "KEY=VALUE")]
        settings: Vec<String>,
    },
    /// Remove a plugin's weights or multipliers: `window.<name>.weight.<class>` or
    /// `window.<name>.multiplier.<glob>`.
    UnsetPlugin {
        provider: String,
        #[arg(required = true, value_name = "KEY")]
        keys: Vec<String>,
    },
    /// Set the amortization length: `window 2h` for the default, `window sonnet 1h` for a target,
    /// `window default` or `window sonnet default` to go back to the default.
    Window {
        #[arg(required = true, num_args = 1..=2, value_name = "[TARGET] DURATION|default")]
        args: Vec<String>,
    },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

pub(crate) use nullrouter_server::ops::routing::edit_key;

fn report(r: Result<Report, OpError>, log: &[String]) -> Result<ExitCode, ExitCode> {
    for l in log {
        eprintln!("{l}");
    }
    let r = r.map_err(|e| {
        eprintln!("{}", e.text);
        ExitCode::from(e.exit)
    })?;
    if let Status::SavedNotApplied { reason } = &r.done.status {
        return Err(fail(reason));
    }
    println!("{}", r.done.text);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn run(home: Option<PathBuf>, args: Args, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let Some(command) = args.command else { return view(&home, args.target.as_deref(), as_json) };
    let ctx = Ctx { home, engine: None };
    let mut log = Vec::new();
    let log = &mut log;
    let result = match command {
        Command::Set { provider, name, settings } => ops::routing::set_with(&ctx, log, &provider, &name, &settings),
        Command::Unset { provider, name, keys } => ops::routing::unset_with(&ctx, log, &provider, &name, &keys),
        Command::SetPlugin { provider, settings } => ops::routing::set_plugin_with(&ctx, log, &provider, &settings),
        Command::UnsetPlugin { provider, keys } => ops::routing::unset_plugin_with(&ctx, log, &provider, &keys),
        Command::Window { args } => {
            let (target, length) = match args.as_slice() {
                [length] => (None, length.as_str()),
                [target, length] => (Some(target.as_str()), length.as_str()),
                _ => return Err(fail("usage: routing window [<target>] <duration|default>")),
            };
            ops::routing::window_with(&ctx, log, target, length)
        }
    };
    report(result, log)
}

fn view(home: &OperatorHome, target: Option<&str>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let view = super::read(home, views::routing::NEEDS, &json!({"target": target}), views::routing::build)?;
    if as_json {
        println!("{:#}", view.json);
    } else {
        print!("{}", routing_text::render(&view.json, nullrouter_engine::clock::now()));
    }
    Ok(ExitCode::SUCCESS)
}
