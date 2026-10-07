//! `nullrouter`: the operator CLI (slice 002 registry commands, slice 003
//! contracts/operator-cli.md). `serve` runs the server; every other command works on the
//! files in `$NULLROUTER_HOME` and, where a server is running, tells it to reload.
//!
//! Exit codes: 0 ok, 1 invalid input or file, 2 usage, 3 plugin not supported by this core,
//! 4 no running server, 5 sign-in refused, expired or abandoned.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use nullrouter_registry::{OperatorHome, RegistryHandle};

mod cmd;

#[derive(Parser)]
#[command(name = "nullrouter", version, about)]
struct Cli {
    /// Operator home. Defaults to `$NULLROUTER_HOME`, else `~/.0router`.
    #[arg(long, global = true, value_name = "DIR")]
    home: Option<PathBuf>,
    /// Machine-readable output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Load everything and print the load report. Exit 1 on any error.
    Check,
    /// Run plugin files through the validation gate. Exit 1 if any is invalid.
    Validate {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Resolve `provider/model`, a unified model or a combo name. Exit 2 if not found.
    Resolve { target: String },
    /// List unified models, or one. Exit 2 if NAME isn't loaded.
    Unified { name: Option<String> },
    /// List combos with their members, or one. Exit 2 if NAME isn't loaded.
    Combos { name: Option<String> },
    /// Show what a provider declares about a model, or, without MODEL, about every model it
    /// declares. Exit 2 if the provider is unknown.
    Model { provider: String, model: Option<String> },
    /// List providers.
    Providers {
        /// Only providers that offer this capability (e.g. `tts`).
        #[arg(long, value_name = "KIND")]
        capability: Option<String>,
    },
    /// Run the server in the foreground. Logs go to stderr, redacted.
    Serve {
        #[arg(long, value_name = "ADDR")]
        listen: Option<String>,
    },
    /// Provider accounts (`accounts.toml`).
    #[command(subcommand)]
    Accounts(cmd::accounts::Command),
    /// Agent keys (`keys.toml`).
    #[command(subcommand)]
    Keys(cmd::keys::Command),
    /// The read-only web dashboard: its token and its state.
    #[command(subcommand)]
    Dashboard(cmd::dashboard::Command),
    /// Operator defaults for request handling.
    #[command(subcommand)]
    Behaviour(cmd::behaviour::Command),
    /// Request records of the running server.
    #[command(subcommand)]
    Records(cmd::records::Command),
    /// Bundled, installed and community plugins.
    #[command(subcommand)]
    Plugins(cmd::plugins::Command),
    /// Provider-reported quota and its poll history.
    Quota(cmd::quota::Args),
    /// The routing view and per-account routing settings.
    Routing(cmd::routing::Args),
    /// Test models with real, billed calls through the running server.
    Test(cmd::test::Args),
}

/// Opens the registry, or prints the startup errors and exits 1.
pub(crate) fn open(home: Option<PathBuf>) -> Result<RegistryHandle, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    nullrouter_server::views::open_registry(&home).map_err(|e| {
        eprintln!("{}", e.message);
        ExitCode::from(e.code)
    })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Check => cmd::check::run(cli.home, cli.json),
        Command::Validate { files } => cmd::validate::run(&files),
        Command::Resolve { target } => cmd::resolve::run(cli.home, &target, cli.json),
        Command::Unified { name } => cmd::unified::run(cli.home, name.as_deref(), cli.json),
        Command::Combos { name } => cmd::combos::run(cli.home, name.as_deref(), cli.json),
        Command::Model { provider, model } => cmd::model::run(cli.home, &provider, model.as_deref(), cli.json),
        Command::Providers { capability } => cmd::providers::run(cli.home, capability.as_deref(), cli.json),
        Command::Serve { listen } => cmd::serve::run(cli.home, listen),
        Command::Accounts(c) => cmd::accounts::run(cli.home, c, cli.json),
        Command::Keys(c) => cmd::keys::run(cli.home, c, cli.json),
        Command::Dashboard(c) => cmd::dashboard::run(cli.home, c, cli.json),
        Command::Behaviour(c) => cmd::behaviour::run(cli.home, c, cli.json),
        Command::Records(c) => cmd::records::run(cli.home, c, cli.json),
        Command::Plugins(c) => cmd::plugins::run(cli.home, c, cli.json),
        Command::Quota(c) => cmd::quota::run(cli.home, c, cli.json),
        Command::Routing(c) => cmd::routing::run(cli.home, c, cli.json),
        Command::Test(c) => cmd::test::run(cli.home, c, cli.json),
    };
    result.unwrap_or_else(|code| code)
}
