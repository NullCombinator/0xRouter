//! `zerorouter behaviour` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Operator default for a stream that breaks after output: `restart` or `error_event`.
    SetBreak { behaviour: String },
}

pub(crate) fn run(_home: Option<PathBuf>, _cmd: Command) -> Result<ExitCode, ExitCode> {
    eprintln!("behaviour: not implemented yet");
    Err(ExitCode::from(1))
}
