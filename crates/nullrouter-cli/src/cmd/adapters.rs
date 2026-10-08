//! `nullrouter adapters` (contracts/operator-cli.md). Until the store arrives with user story 2,
//! only `list` works: it shows the built-in adapters. The other subcommands say so.

use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_adapters::BUILTIN;
use serde_json::json;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Each harness with an adapter, its versions and state. hermes shows as `built-in`.
    List,
    /// `show`, `install`, `review`, `build`, `approve`, `reject`, `clear`, `remove`,
    /// `rebuild` and `review-settings` arrive with third-party adapters.
    #[command(external_subcommand)]
    Other(Vec<String>),
}

pub(crate) fn run(cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    match cmd {
        Command::List => {
            if as_json {
                let rows: Vec<_> = BUILTIN
                    .iter()
                    .map(|h| json!({"harness": h, "built_in": true, "versions": [], "active": "builtin", "alerts": 0}))
                    .collect();
                println!("{}", serde_json::Value::Array(rows));
            } else {
                for h in BUILTIN {
                    println!("{h}  built-in");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Other(args) => {
            eprintln!("nullrouter adapters {}: not available yet", args.first().map_or("", String::as_str));
            Err(ExitCode::from(1))
        }
    }
}
