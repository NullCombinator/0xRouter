//! `zerorouter plugins` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use serde_json::{Value, json};
use zerorouter_registry::OperatorHome;
use zerorouter_registry::community::{self, InstallError};

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

/// Exit code for a plugin this core can't support.
const UNSUPPORTED: u8 = 3;

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    match cmd {
        Command::List { community } => list(home, community, as_json),
        Command::Install { id } => {
            let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
            let path = community::install(&id, &home).map_err(refused)?;
            done(&home, "installed", &id, &path, as_json)
        }
        Command::Uninstall { id } => {
            let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
            let path = community::uninstall(&id, &home).map_err(refused)?;
            done(&home, "uninstalled", &id, &path, as_json)
        }
    }
}

fn refused(e: InstallError) -> ExitCode {
    eprintln!("{e}");
    match e {
        InstallError::Unsupported(_) => ExitCode::from(UNSUPPORTED),
        _ => ExitCode::from(1),
    }
}

fn done(
    home: &OperatorHome,
    what: &str,
    id: &str,
    path: &std::path::Path,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    let status = super::apply(home).map_err(|e| {
        eprintln!("{what} {id} ({}), {e}", path.display());
        ExitCode::from(1)
    })?;
    if as_json {
        println!("{}", json!({"id": id, what: path.display().to_string(), "status": status}));
    } else {
        println!("{what} {id} ({}), {status}", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

/// Bundled and installed plugins as loaded, then, with `--community`, every community
/// plugin with its fit status.
fn list(home: Option<PathBuf>, with_community: bool, as_json: bool) -> Result<ExitCode, ExitCode> {
    let handle = crate::open(home)?;
    let reg = handle.snapshot();
    let mut rows: Vec<Value> = reg
        .providers()
        .map(|p| json!({"id": p.id, "set": if p.is_bundled() { "bundled" } else { "user" }, "status": "loaded"}))
        .collect();
    let report = reg.report();
    rows.extend(report.unsupported.iter().map(|u| json!({"id": u.id, "set": "user", "status": "unsupported"})));
    rows.extend(report.skipped.iter().map(|s| json!({"id": s.id, "set": "user", "status": "invalid"})));
    if with_community {
        for p in community::community() {
            let status = match &p.verdict {
                _ if community::is_installed(p.id, handle.home()) => "installed",
                Ok(v) if v.fits() => "fits",
                Ok(_) => "unsupported",
                Err(_) => "invalid",
            };
            rows.push(json!({"id": p.id, "set": "community", "status": status}));
        }
    }
    if as_json {
        println!("{:#}", json!(rows));
    } else {
        for r in &rows {
            let s = |k: &str| r[k].as_str().unwrap_or_default().to_owned();
            println!("{:<24} {:<10} {}", s("id"), s("set"), s("status"));
        }
    }
    Ok(ExitCode::SUCCESS)
}
