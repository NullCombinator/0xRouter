//! The 0router provider and unified-model registry (spec 002).
//!
//! Providers are declared by TOML plugins: bundled ones are embedded at build time, user
//! ones are read from `$NULLROUTER_HOME/plugins/`. Plugins are data, never code, and never
//! carry secrets. The operator declares unified models in `$NULLROUTER_HOME/config.toml`.
//!
//! [`RegistryHandle`] owns the active [`Registry`] snapshot. Readers take a snapshot per
//! request; [`RegistryHandle::reload`] builds and validates a full candidate before
//! swapping it in.

use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;

pub mod community;
mod combos;
mod convert;
mod credentials;
pub mod fit;
pub mod floor;
mod load;
pub mod logo;
mod lookup;
mod registry;
mod resolve;
pub mod schema;
pub mod template;
pub mod validate;
mod views;

pub use combos::{Combo, ComboStep, DroppedCombo};
pub use credentials::{ResolvedCredential, SecretString};
#[cfg(feature = "parity")]
pub use load::parity_set;
pub use load::{
    DroppedUnifiedModel, IgnoredLogo, LimitsNote, LoadReport, OperatorHome, PluginConflict, ReloadError, SkippedPlugin,
    StartupError, UnsupportedPlugin, WithheldCredential, bundled_gate_ctx, bundled_style_sources, check_user_plugin,
    load_config, validate_user_plugin,
};
pub use logo::Logo;
pub use lookup::{derive_model_name, normalise_version_sep, split_suffix};
pub use registry::{CatalogEntry, ModelInfo, Registry, RuntimeSettings, UnifiedMember, UnifiedModel};
pub use resolve::{NotFound, Resolution};
pub use schema::{CapabilityKind, ModelKind, PluginSource, ProviderEntity};
pub use validate::ValidationError;
pub use views::{ComposedTransport, OAuthUrlsView};

/// The embedded bundled plugins as `(file name, source)`, in file-name order.
pub fn bundled_sources() -> &'static [(&'static str, &'static str)] {
    load::BUNDLED
}

/// Process-wide registry handle. Cheap to clone; reads are lock-free.
#[derive(Clone)]
pub struct RegistryHandle {
    inner: Arc<Inner>,
}

struct Inner {
    active: ArcSwap<Registry>,
    reload: Mutex<()>,
    home: OperatorHome,
    parity: bool,
}

impl RegistryHandle {
    /// Loads bundled plugins, user plugins, and `config.toml`.
    ///
    /// Fatal: an invalid bundled plugin or an invalid `config.toml`. Invalid user plugins
    /// are skipped and listed in [`Registry::report`].
    pub fn open(home: OperatorHome) -> Result<Self, StartupError> {
        Self::open_set(home, false)
    }

    /// [`open`](Self::open) with the community plugins loaded as bundled and the fit check
    /// off, as 9router ships them: for slice 002's tests, which need its 121 providers.
    #[cfg(feature = "parity")]
    pub fn open_parity(home: OperatorHome) -> Result<Self, StartupError> {
        Self::open_set(home, true)
    }

    fn open_set(home: OperatorHome, parity: bool) -> Result<Self, StartupError> {
        let registry = load::build(&home, load::Mode::Startup, parity).map_err(|errors| StartupError { errors })?;
        let inner = Inner { active: ArcSwap::from_pointee(registry), reload: Mutex::new(()), home, parity };
        Ok(Self { inner: Arc::new(inner) })
    }

    /// A consistent snapshot. Hold it for the whole request (FR-025).
    pub fn snapshot(&self) -> Arc<Registry> {
        self.inner.active.load_full()
    }

    pub fn home(&self) -> &OperatorHome {
        &self.inner.home
    }

    /// Rebuilds everything from disk, validates it, then swaps it in atomically (FR-024).
    /// On any error the active snapshot is unchanged. There is no file watching (FR-026).
    ///
    /// This does blocking file I/O: async callers should use `spawn_blocking`.
    pub fn reload(&self) -> Result<LoadReport, ReloadError> {
        let _guard = self.inner.reload.lock().unwrap_or_else(|e| e.into_inner());
        let candidate = load::build(&self.inner.home, load::Mode::Reload, self.inner.parity)
            .map_err(|errors| ReloadError { errors })?;
        let report = candidate.report().clone();
        self.inner.active.store(Arc::new(candidate));
        Ok(report)
    }
}
