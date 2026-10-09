//! `nullrouter-builder`: `setup` prepares the build environment, `build` reads a JSON job from
//! stdin and compiles the adapter source to WASM.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use nullrouter_builder::{BuildError, Job, build, builder_dir, default_home, setup};

#[derive(Parser)]
#[command(name = "nullrouter-builder", version)]
struct Cli {
    /// The 0router home (default: `$NULLROUTER_HOME`, else `~/.0router`). The builder lives in
    /// its `builder/` directory.
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Prepare the builder's toolchain and offline dependency cache (uses the network once).
    Setup,
    /// Read a JSON job from stdin and build it; the JSON result goes to stdout.
    Build,
}

fn run(cli: Cli) -> Result<(), BuildError> {
    let dir = builder_dir(&cli.home.unwrap_or_else(default_home));
    match cli.cmd {
        Cmd::Setup => setup(&dir),
        Cmd::Build => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input).map_err(|e| BuildError::io("reading the job", e))?;
            let job: Job = serde_json::from_str(&input)?;
            let result = build(&dir, &job)?;
            println!("{}", serde_json::to_string(&result)?);
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nullrouter-builder: {e}");
            ExitCode::FAILURE
        }
    }
}
