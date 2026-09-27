//! `zerorouter-cli`: the operator's view of the registry (contracts/registry-api.md § CLI).
//! It never serves requests.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use zerorouter_registry::{OperatorHome, RegistryHandle};

mod cmd;

#[derive(Parser)]
#[command(name = "zerorouter-cli", version, about)]
struct Cli {
    /// Operator home. Defaults to `$ZEROROUTER_HOME`, else `~/.0router`.
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
    /// Resolve `provider/model` or a unified model name. Exit 2 if not found.
    Resolve { target: String },
    /// Show what a provider declares about a model. Exit 2 if the provider is unknown.
    Model { provider: String, model: String },
    /// List providers.
    Providers {
        /// Only providers that offer this capability (e.g. `tts`).
        #[arg(long, value_name = "KIND")]
        capability: Option<String>,
    },
}

/// Opens the registry, or prints the startup errors and exits 1.
pub(crate) fn open(home: Option<PathBuf>) -> Result<RegistryHandle, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    RegistryHandle::open(home).map_err(|e| {
        eprintln!("startup failed:\n{e}");
        ExitCode::from(1)
    })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Check => cmd::check::run(cli.home, cli.json),
        Command::Validate { files } => cmd::validate::run(&files),
        Command::Resolve { target } => cmd::resolve::run(cli.home, &target, cli.json),
        Command::Model { provider, model } => cmd::model::run(cli.home, &provider, &model, cli.json),
        Command::Providers { capability } => cmd::providers::run(cli.home, capability.as_deref(), cli.json),
    };
    result.unwrap_or_else(|code| code)
}
