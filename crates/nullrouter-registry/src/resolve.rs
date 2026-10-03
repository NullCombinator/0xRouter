//! Target classification (FR-014a, FR-016, FR-017): `provider/model` is direct, a bare
//! name is a unified model. A provider is never inferred from a bare name.

use crate::registry::{Registry, UnifiedModel};
use crate::schema::ProviderEntity;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotFound {
    #[error("unknown provider {token:?}")]
    Provider { token: String },
    #[error("no unified model named {name:?}")]
    UnifiedModel { name: String },
    #[error("provider {provider:?} does not declare model {model:?} and uncatalogued models are disabled")]
    Model { provider: String, model: String },
    #[error("empty target: expected \"provider/model\" or a unified model name")]
    EmptyTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Resolution<'a> {
    Direct {
        provider: &'a ProviderEntity,
        /// The model part of the target, as sent.
        requested: &'a str,
        upstream_id: String,
        /// `false`: not in the catalog, forwarded because the provider is passthrough or
        /// allows uncatalogued models.
        catalogued: bool,
    },
    Unified(&'a UnifiedModel),
}

impl Registry {
    /// Classifies `target` by shape. The only allocation on success is the upstream ID.
    pub fn resolve<'a>(&'a self, target: &'a str) -> Result<Resolution<'a>, NotFound> {
        self.resolve_with(target, |_, _| false)
    }

    /// [`resolve`](Self::resolve), with `live(provider_id, model)` naming the models a
    /// provider's live list (`[models_live]`) holds beside its static catalog. The registry
    /// stays file-only: the caller owns the live lists. A live model resolves as an
    /// uncatalogued direct target even when uncatalogued models are disabled; static
    /// results are unchanged.
    pub fn resolve_with<'a>(
        &'a self,
        target: &'a str,
        live: impl FnOnce(&str, &str) -> bool,
    ) -> Result<Resolution<'a>, NotFound> {
        if target.is_empty() {
            return Err(NotFound::EmptyTarget);
        }
        let Some((token, model)) = target.split_once('/') else {
            return self.unified_model(target).map(Resolution::Unified);
        };
        if token.is_empty() || model.is_empty() {
            return Err(NotFound::EmptyTarget);
        }
        let p = self.index_of(token).ok_or_else(|| NotFound::Provider { token: token.to_owned() })?;
        let provider = &self.providers[p];
        let (found, upstream_id) = self.lookup_at(p, model);
        let catalogued = found.is_some();
        if !catalogued
            && !provider.passthrough_models
            && !self.settings(&provider.id).allow_uncatalogued_models
            && !live(&provider.id, model)
        {
            return Err(NotFound::Model { provider: provider.id.clone(), model: model.to_owned() });
        }
        Ok(Resolution::Direct { provider, requested: model, upstream_id, catalogued })
    }
}
