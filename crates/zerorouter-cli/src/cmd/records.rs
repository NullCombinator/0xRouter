//! `zerorouter records` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Newest first.
    List {
        #[arg(long)]
        provider: Option<String>,
        /// Unified model name.
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
    },
    Show { id: String },
}

pub(crate) fn run(_home: Option<PathBuf>, _cmd: Command, _json: bool) -> Result<ExitCode, ExitCode> {
    eprintln!("records: not implemented yet");
    Err(ExitCode::from(1))
}
