//! `zerorouter serve` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use zerorouter_engine::redact::RedactWriter;
use zerorouter_engine::state::Engine;
use zerorouter_registry::OperatorHome;
use zerorouter_server::operator;
use zerorouter_server::serve::{self, App};

pub(crate) fn run(home: Option<PathBuf>, listen: Option<String>) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let (engine, report) = Engine::open(home).map_err(|e| {
        eprintln!("startup failed:\n{e}");
        ExitCode::from(1)
    })?;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(RedactWriter::new(engine.redactor(), std::io::stderr))
        .try_init();
    for a in &report.unused_accounts {
        tracing::warn!("account {a} names a provider that isn't loaded");
    }
    let listen = listen.unwrap_or_else(|| engine.snapshot().settings().server.listen.clone());
    let home = engine.home().clone();
    let engine = Arc::new(engine);
    let app = App::new(engine.clone()).map_err(|e| {
        eprintln!("startup failed: {e}");
        ExitCode::from(1)
    })?;
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| {
        eprintln!("startup failed: {e}");
        ExitCode::from(1)
    })?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(&listen).await.map_err(|e| {
            eprintln!("cannot listen on {listen}: {e}");
            ExitCode::from(1)
        })?;
        let socket = operator::bind(&home).map_err(|e| {
            eprintln!("cannot open the operator socket: {e}");
            ExitCode::from(1)
        })?;
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let ops = tokio::spawn(operator::serve(engine, socket, async move {
            let mut stopped = stopped;
            let _ = stopped.wait_for(|s| *s).await;
        }));
        tracing::info!("listening on {listen}");
        let served = serve::run(app, listener, serve::signal()).await;
        let _ = stop.send(true);
        let _ = ops.await;
        served.map_err(|e| {
            eprintln!("server failed: {e}");
            ExitCode::from(1)
        })?;
        tracing::info!("stopped");
        Ok(ExitCode::SUCCESS)
    })
}
