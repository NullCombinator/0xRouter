//! `nullrouter-builder`: `setup` prepares the build environment, `build` reads a JSON job from
//! stdin and compiles the adapter source to WASM.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "nullrouter-builder", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Prepare the builder's toolchain and offline dependency cache.
    Setup,
    /// Read a JSON job from stdin and build it.
    Build,
}

fn main() {
    match Cli::parse().cmd {
        Cmd::Setup | Cmd::Build => {}
    }
}
