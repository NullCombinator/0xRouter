use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args as ClapArgs, Subcommand};
use nullrouter_registry::OperatorHome;
use nullrouter_server::ops::plugins;
use nullrouter_server::ops::{Ctx, Status};
use nullrouter_server::views;
use serde_json::json;

/// `nullrouter providers [--capability KIND]` lists; `add-compatible` declares a provider.
#[derive(ClapArgs)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct Args {
    /// Only providers that offer this capability (e.g. `tts`).
    #[arg(long, value_name = "KIND")]
    pub(crate) capability: Option<String>,
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Add a server that speaks the Chat Completions or Messages API as a provider.
    AddCompatible {
        id: String,
        #[arg(long, value_parser = ["openai", "anthropic"])]
        style: String,
        #[arg(long, value_name = "URL")]
        base_url: String,
        /// A model id the server serves; repeat for each.
        #[arg(long = "model", value_name = "ID", required = true)]
        models: Vec<String>,
        /// Shown name; the id when omitted.
        #[arg(long)]
        name: Option<String>,
    },
}

pub(crate) fn run(home: Option<PathBuf>, args: Args, as_json: bool) -> Result<ExitCode, ExitCode> {
    if let Some(Command::AddCompatible { id, style, base_url, models, name }) = args.command {
        let ctx = Ctx { home: home.map_or_else(OperatorHome::resolve, OperatorHome::new), engine: None };
        let mut log = Vec::new();
        let result = plugins::add_compatible_with(&ctx, &mut log, &id, name.as_deref(), &style, &base_url, &models);
        for l in &log {
            eprintln!("{l}");
        }
        let fail = |text: String, code: u8| {
            eprintln!("{text}");
            ExitCode::from(code)
        };
        let r = result.map_err(|e| fail(e.text, e.exit))?;
        if let Status::SavedNotApplied { reason } = &r.done.status {
            return Err(fail(reason.clone(), 1));
        }
        if as_json {
            println!("{}", r.json);
        } else {
            println!("{}", r.done.text);
        }
        return Ok(ExitCode::SUCCESS);
    }
    list(home, args.capability.as_deref(), as_json)
}

fn list(home: Option<PathBuf>, capability: Option<&str>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let view =
        super::read(&home, views::providers::NEEDS, &json!({"capability": capability}), views::providers::build)?;
    let rows = view.json.as_array().map_or(&[][..], Vec::as_slice);
    if as_json {
        println!("{:#}", view.json);
    } else {
        for r in rows {
            println!(
                "{:<24} {:<10} {:<10} {}",
                r["id"].as_str().unwrap_or_default(),
                r["alias"].as_str().unwrap_or("-"),
                r["category"].as_str().unwrap_or_default(),
                r["capabilities"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}
