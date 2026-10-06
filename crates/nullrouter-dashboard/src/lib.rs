//! The read-only web dashboard served by `nullrouter serve` on its own loopback port.
//!
//! Pages are server-rendered from the same read model the CLI prints (`nullrouter_server::views`);
//! nothing here changes state. Routes, headers and access rules:
//! `specs/009-dashboard/contracts/dashboard-http.md`.
//!
//! The dashboard runs beside the client listener and can't slow or break it: its own listener,
//! its own task, bounded page builds ([`guard`]), and a bind failure that is only recorded.

use std::future::Future;
use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use nullrouter_engine::state::Engine;
use nullrouter_engine::status::DashboardStatus;
use nullrouter_registry::schema::DashboardSettings;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub mod guard;
pub mod headers;
pub mod page;
pub mod time;

/// What every handler shares.
pub struct Shared {
    pub engine: Arc<Engine>,
    /// The version `nullrouter --version` prints, for the sidebar.
    pub version: String,
    pub guard: guard::Guard,
}

/// A running dashboard (or the record of why there isn't one).
pub struct DashboardHandle {
    status: DashboardStatus,
    task: Option<JoinHandle<()>>,
}

impl DashboardHandle {
    /// What `server.status` reports.
    pub fn status(&self) -> &DashboardStatus {
        &self.status
    }

    /// Resolves once the listener has stopped (immediately when it never started).
    pub async fn stopped(self) {
        if let Some(task) = self.task {
            let _ = task.await;
        }
    }
}

/// Binds `settings.listen` and serves until `shutdown` resolves. The bound state, or the reason
/// the bind failed, goes to `engine.status` for the operator socket. A bind failure is logged and
/// returned in the handle; it never stops `serve`.
pub async fn spawn(
    engine: Arc<Engine>,
    settings: &DashboardSettings,
    version: impl Into<String>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> DashboardHandle {
    let mut status = DashboardStatus { enabled: true, listen: settings.listen.clone(), serving: false, error: None };
    let listener = match TcpListener::bind(&settings.listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("the dashboard is not listening on {}: {e}", settings.listen);
            status.error = Some(format!("{}: {e}", settings.listen));
            engine.status.set_dashboard(status.clone());
            return DashboardHandle { status, task: None };
        }
    };
    let port = listener.local_addr().map_or(0, |a| a.port());
    let host = settings.listen.rsplit_once(':').map_or(settings.listen.as_str(), |(h, _)| h);
    let shared = Arc::new(Shared { engine: engine.clone(), version: version.into(), guard: guard::Guard::default() });
    let app = router(shared, headers::HostRule::new(host, port));
    status.serving = true;
    engine.status.set_dashboard(status.clone());
    tracing::info!("the dashboard is listening on {}", settings.listen);
    let task = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown).await {
            tracing::warn!("the dashboard stopped: {e}");
        }
    });
    DashboardHandle { status, task: Some(task) }
}

/// Every route, behind the Host and method checks and the response headers.
pub fn router(shared: Arc<Shared>, rule: headers::HostRule) -> Router {
    Router::new()
        .fallback(not_built)
        .with_state(shared)
        .layer(axum::middleware::from_fn_with_state(rule, headers::enforce))
}

async fn not_built() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "No such page.\n")
}
