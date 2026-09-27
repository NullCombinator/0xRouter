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
        /// `false`: undeclared, allowed by `allow_uncatalogued_models`.
        catalogued: bool,
    },
    Unified(&'a UnifiedModel),
}

impl Registry {
    /// Classifies `target` by shape. The only allocation on success is the upstream ID.
    pub fn resolve<'a>(&'a self, target: &'a str) -> Result<Resolution<'a>, NotFound> {
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
        let catalogued = provider.passthrough_models || self.find_at(p, model).is_some();
        if !catalogued && !self.settings(&provider.id).allow_uncatalogued_models {
            return Err(NotFound::Model { provider: provider.id.clone(), model: model.to_owned() });
        }
        Ok(Resolution::Direct { provider, requested: model, upstream_id: self.upstream_at(p, model), catalogued })
    }
}
