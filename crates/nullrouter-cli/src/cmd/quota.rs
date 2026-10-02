//! `nullrouter quota` (slice 005 contracts/operator-cli.md § quota): provider-reported quota per
//! account, its poll history, and the polling interval.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Current windows per account, reset times, last poll and last failure.
    Show {
        provider: Option<String>,
        name: Option<String>,
    },
    /// Poll entries of one account, with the traffic tally of each interval.
    History {
        provider: String,
        name: String,
        /// RFC 3339 date or time.
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Poll one account now (needs a running server).
    Poll { provider: String, name: String },
    /// Set an account's polling interval (`15m`, `1h`, or `default`).
    Interval {
        provider: String,
        name: String,
        every: String,
    },
    /// Delete history entries older than DATE.
    Prune {
        #[arg(long, value_name = "DATE")]
        before: String,
        provider: Option<String>,
        name: Option<String>,
    },
    /// Delete one account's history.
    Forget { provider: String, name: String },
}

pub(crate) fn run(_home: Option<PathBuf>, _cmd: Command, _json: bool) -> Result<ExitCode, ExitCode> {
    eprintln!("quota: not available in this build yet");
    Err(ExitCode::from(1))
}
