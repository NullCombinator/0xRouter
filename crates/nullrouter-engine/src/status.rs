//! What the running process knows about its own listeners, for the operator socket's
//! `server.status` op (spec 009, R8). Set by `serve` once the listeners are bound; a process
//! that never serves (a test's engine) leaves it unset.

use std::sync::{Mutex, OnceLock};

/// The dashboard listener's state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DashboardStatus {
    /// `[dashboard] enabled` in `config.toml`.
    pub enabled: bool,
    /// The address from `[dashboard] listen`.
    pub listen: String,
    /// Whether the listener is bound and serving.
    pub serving: bool,
    /// Why it isn't, when it is enabled and not serving.
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct ServerStatus {
    client_listen: OnceLock<String>,
    dashboard: Mutex<DashboardStatus>,
}

impl ServerStatus {
    /// The address the client listener actually bound (`serve --listen` can differ from the file).
    pub fn set_client_listen(&self, addr: impl Into<String>) {
        let _ = self.client_listen.set(addr.into());
    }

    pub fn client_listen(&self) -> Option<&str> {
        self.client_listen.get().map(String::as_str)
    }

    pub fn set_dashboard(&self, status: DashboardStatus) {
        *self.dashboard.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = status;
    }

    pub fn dashboard(&self) -> DashboardStatus {
        self.dashboard.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_until_serve_sets_it() {
        let s = ServerStatus::default();
        assert_eq!(s.client_listen(), None);
        assert_eq!(s.dashboard(), DashboardStatus::default());
        s.set_client_listen("127.0.0.1:20129");
        s.set_dashboard(DashboardStatus {
            enabled: true,
            listen: "127.0.0.1:20130".into(),
            serving: false,
            error: Some("address in use".into()),
        });
        assert_eq!(s.client_listen(), Some("127.0.0.1:20129"));
        assert_eq!(s.dashboard().error.as_deref(), Some("address in use"));
    }
}
