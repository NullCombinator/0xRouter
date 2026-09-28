//! `zerorouter keys` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Issue a key. It is printed once; only its digest is stored.
    Issue {
        name: String,
        #[arg(long = "break", value_name = "BEHAVIOUR")]
        break_behaviour: Option<String>,
    },
    List,
    /// Revoke a key by name or id.
    Revoke { key: String },
    /// Per-key break behaviour: `restart`, `error_event`, or `default`.
    SetBreak { key: String, behaviour: String },
}

pub(crate) fn run(_home: Option<PathBuf>, _cmd: Command, _json: bool) -> Result<ExitCode, ExitCode> {
    eprintln!("keys: not implemented yet");
    Err(ExitCode::from(1))
}
