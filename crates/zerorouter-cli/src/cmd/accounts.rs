//! `zerorouter accounts` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;

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
    /// List accounts, optionally for one provider.
    List { provider: Option<String> },
    Remove { provider: String, name: String },
    Disable { provider: String, name: String },
    Enable { provider: String, name: String },
}

pub(crate) fn run(_home: Option<PathBuf>, _cmd: Command, _json: bool) -> Result<ExitCode, ExitCode> {
    eprintln!("accounts: not implemented yet");
    Err(ExitCode::from(1))
}
