//! `nullrouter serve` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use nullrouter_engine::maintenance;
use nullrouter_engine::redact::RedactWriter;
use nullrouter_engine::state::Engine;
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator;
use nullrouter_server::serve::{self, App};

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
    // An `adapters/` other users can enter is refused here, before anything listens.
    engine.open_adapters().map_err(|e| {
        eprintln!("startup failed: {e}");
        ExitCode::from(1)
    })?;
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
        if let Ok(addr) = listener.local_addr() {
            engine.status.set_client_listen(addr.to_string());
        }
        // The operator socket appearing means the server is ready, so the journal is made whole
        // first: until then no client of the socket can read a request a crash left open as
        // still in flight.
        serve::recover_journal(&engine).await;
        let socket = operator::bind(&home).map_err(|e| {
            eprintln!("cannot open the operator socket: {e}");
            ExitCode::from(1)
        })?;
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let until_stopped = |mut stopped: tokio::sync::watch::Receiver<bool>| async move {
            let _ = stopped.wait_for(|s| *s).await;
        };
        // Token refreshes, quota polls and live model lists run while the server is up
        // (research R11, R13, R14).
        let upkeep = maintenance::spawn(engine.clone(), until_stopped(stopped.clone()));
        let journal_owner = engine.clone();
        // The dashboard has its own listener and task; a bind failure is recorded, not fatal
        // (spec 009).
        let settings = engine.snapshot().settings().dashboard.clone();
        let dashboard = nullrouter_dashboard::spawn(
            engine.clone(),
            &settings,
            env!("CARGO_PKG_VERSION"),
            until_stopped(stopped.clone()),
        )
        .await;
        let ops = tokio::spawn(operator::serve(engine, socket, until_stopped(stopped)));
        tracing::info!("listening on {listen}");
        let served = serve::serve_recovered(app, listener, serve::signal()).await;
        let _ = stop.send(true);
        let _ = ops.await;
        dashboard.stopped().await;
        let _ = upkeep.await;
        // Every line sent so far is written and synced before the process exits (spec 006).
        let _ = tokio::task::spawn_blocking(move || journal_owner.journal.shutdown()).await;
        served.map_err(|e| {
            eprintln!("server failed: {e}");
            ExitCode::from(1)
        })?;
        tracing::info!("stopped");
        Ok(ExitCode::SUCCESS)
    })
}
