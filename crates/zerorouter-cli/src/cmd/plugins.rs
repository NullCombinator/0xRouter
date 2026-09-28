//! `zerorouter plugins` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum Command {
    List {
        /// Include the community set with its fit status.
        #[arg(long)]
        community: bool,
    },
    /// Install a community plugin. Exit 3 if this core can't support it.
    Install { id: String },
    Uninstall { id: String },
}

pub(crate) fn run(_home: Option<PathBuf>, _cmd: Command, _json: bool) -> Result<ExitCode, ExitCode> {
    eprintln!("plugins: not implemented yet");
    Err(ExitCode::from(1))
}
