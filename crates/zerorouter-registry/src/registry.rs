//! One immutable snapshot of the provider set and operator state (data-model § Registry).

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use crate::credentials::{self, ResolvedCredential};
use crate::load::{LoadReport, WithheldCredential};
use crate::lookup::{Catalog, derive_model_name};
use crate::resolve::NotFound;
use crate::schema::{
    CapabilityKind, CapabilitySection, ContentKind, Model, ModelKind, ProviderEntity, ProviderSettings, SectionModel,
    WireFormat,
};
use crate::validate::FieldPath;

/// An operator-declared routing target (FR-014 – FR-016).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedModel {
    pub name: String,
    pub kind: Option<ModelKind>,
    /// Never empty. In declaration order.
    pub members: Vec<UnifiedMember>,
}

/// A member resolved once, at load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedMember {
    /// Canonical provider id (the member may have named an alias).
    pub provider: String,
    /// The model id as written in `config.toml`.
    pub requested: String,
    /// FR-021.
    pub upstream_id: String,
    /// `false` only for an undeclared model on a passthrough provider.
    pub catalogued: bool,
}

/// The answer to a model query (FR-022). Every field is optional so "not declared" can be
/// told apart from a declared value.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo<'a> {
    /// 9router `isValidModel`: declared, or the provider is passthrough.
    pub declared: bool,
    pub name: Cow<'a, str>,
    /// `None` for an untyped model, never a guessed `llm` (FR-004).
    pub kind: Option<ModelKind>,
    pub target_format: Option<WireFormat>,
    pub supported_formats: Option<&'a [WireFormat]>,
    pub quota_family: Option<&'a str>,
    pub strip: Option<&'a [ContentKind]>,
    pub upstream_id: String,
    /// The declared entry, if any.
    pub model: Option<&'a Model>,
}

/// One entry of a capability section's catalog view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CatalogEntry<'a> {
    Provider(&'a Model),
    Section(&'a SectionModel),
}

impl CatalogEntry<'_> {
    pub fn id(&self) -> &str {
        match self {
            Self::Provider(m) => &m.id,
            Self::Section(m) => &m.id,
        }
    }
}

#[derive(Debug)]
pub struct Registry {
    pub(crate) providers: Vec<ProviderEntity>,
    alias_index: HashMap<Box<str>, usize>,
    catalogs: Vec<Catalog>,
    pub(crate) credentials: HashMap<String, ResolvedCredential>,
    unified: Vec<UnifiedModel>,
    unified_index: HashMap<Box<str>, usize>,
    settings: BTreeMap<String, ProviderSettings>,
    report: LoadReport,
}

/// A lookup token claimed by two providers.
pub(crate) struct TokenClash {
    pub(crate) token: String,
    pub(crate) first: usize,
    pub(crate) second: usize,
}

/// Every token (id, `alias`, `aliases`) claimed by more than one provider.
pub(crate) fn token_clashes(providers: &[&ProviderEntity]) -> Vec<TokenClash> {
    let mut owner: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::new();
    for (i, p) in providers.iter().enumerate() {
        for t in p.tokens() {
            match owner.get(t) {
                Some(&j) if j != i => out.push(TokenClash { token: t.to_owned(), first: j, second: i }),
                Some(_) => {}
                None => {
                    owner.insert(t, i);
                }
            }
        }
    }
    out
}

/// The field of `p` that declares `token`.
pub(crate) fn token_path(p: &ProviderEntity, token: &str) -> FieldPath {
    if p.id == token {
        FieldPath::of("id")
    } else if p.alias.as_deref() == Some(token) {
        FieldPath::of("alias")
    } else {
        let i = p.aliases.iter().position(|a| a == token).unwrap_or(0);
        FieldPath::of("aliases").index(i)
    }
}

impl Registry {
    /// Indexes `providers`. The loader has already rejected token clashes; if any remain,
    /// the first claimant wins.
    pub(crate) fn new(providers: Vec<ProviderEntity>) -> Self {
        let mut alias_index = HashMap::new();
        for (i, p) in providers.iter().enumerate() {
            for t in p.tokens() {
                alias_index.entry(t.into()).or_insert(i);
            }
        }
        let catalogs = providers
            .iter()
            .map(|p| Catalog::new(p.models.as_deref().unwrap_or_default(), p.version_separator_tolerance))
            .collect();
        let credentials = credentials::TABLE
            .iter()
            .filter_map(|entry| {
                let p = providers.iter().find(|p| p.id == entry.provider_id)?;
                Some((p.id.clone(), credentials::bind(entry, p)))
            })
            .collect();
        Self {
            providers,
            alias_index,
            catalogs,
            credentials,
            unified: Vec::new(),
            unified_index: HashMap::new(),
            settings: BTreeMap::new(),
            report: LoadReport::default(),
        }
    }

    pub(crate) fn set_operator_state(
        &mut self,
        unified: Vec<UnifiedModel>,
        settings: BTreeMap<String, ProviderSettings>,
        report: LoadReport,
    ) {
        self.unified_index = unified.iter().enumerate().map(|(i, u)| (u.name.as_str().into(), i)).collect();
        self.unified = unified;
        self.settings = settings;
        self.report = report;
    }

    /// Credentials withheld under FR-012a, in provider order.
    pub(crate) fn withheld_credentials(&self) -> Vec<WithheldCredential> {
        self.providers
            .iter()
            .filter_map(|p| match self.credentials.get(&p.id)? {
                ResolvedCredential::Withheld { offending_url } => {
                    Some(WithheldCredential { provider: p.id.clone(), offending_url: offending_url.clone() })
                }
                ResolvedCredential::Available(_) => None,
            })
            .collect()
    }

    pub(crate) fn index_of(&self, token: &str) -> Option<usize> {
        self.alias_index.get(token).copied()
    }

    fn models_at(&self, p: usize) -> &[Model] {
        self.providers[p].models.as_deref().unwrap_or_default()
    }

    pub(crate) fn find_at(&self, p: usize, model_id: &str) -> Option<&Model> {
        self.catalogs[p].find(self.models_at(p), model_id)
    }

    pub(crate) fn upstream_at(&self, p: usize, model_id: &str) -> String {
        self.catalogs[p].upstream_id(self.models_at(p), model_id)
    }

    fn provider_index(&self, token: &str) -> Result<usize, NotFound> {
        self.index_of(token).ok_or_else(|| NotFound::Provider { token: token.to_owned() })
    }

    /// By id, `alias`, or `aliases` entry; never `ui_alias` (FR-018).
    pub fn provider(&self, token: &str) -> Result<&ProviderEntity, NotFound> {
        self.provider_index(token).map(|i| &self.providers[i])
    }

    pub fn providers(&self) -> impl Iterator<Item = &ProviderEntity> {
        self.providers.iter()
    }

    /// `Ok(None)` means the provider exists but does not offer `kind` (FR-003).
    pub fn capability(&self, provider: &str, kind: CapabilityKind) -> Result<Option<&CapabilitySection>, NotFound> {
        self.provider(provider).map(|p| p.capabilities.get(&kind))
    }

    /// The models a capability section offers: provider models of that kind plus the
    /// section's own list. `llm` also lists untyped models, which keep `kind == None`.
    pub fn catalog(&self, provider: &str, kind: CapabilityKind) -> Result<Vec<CatalogEntry<'_>>, NotFound> {
        let p = self.provider(provider)?;
        let own = p
            .models
            .iter()
            .flatten()
            .filter(|m| m.kind == Some(kind) || (kind == CapabilityKind::Llm && m.kind.is_none()))
            .map(CatalogEntry::Provider);
        let section = p
            .capabilities
            .get(&kind)
            .into_iter()
            .flat_map(|s| s.models.iter().flatten())
            .map(CatalogEntry::Section);
        Ok(own.chain(section).collect())
    }

    /// 9router's per-model queries in one view (FR-019 – FR-022).
    pub fn model(&self, provider: &str, model_id: &str) -> Result<ModelInfo<'_>, NotFound> {
        let p = self.provider_index(provider)?;
        let found = self.find_at(p, model_id);
        let name = match found {
            Some(m) => m.name.as_deref().map_or_else(|| Cow::Owned(derive_model_name(&m.id)), Cow::Borrowed),
            None => Cow::Owned(model_id.to_owned()),
        };
        Ok(ModelInfo {
            declared: found.is_some() || self.providers[p].passthrough_models,
            name,
            kind: found.and_then(|m| m.kind),
            target_format: found.and_then(|m| m.target_format),
            supported_formats: found.and_then(|m| m.supported_formats.as_deref()),
            quota_family: found.and_then(|m| m.quota_family.as_deref()),
            strip: found.and_then(|m| m.strip.as_deref()),
            upstream_id: self.upstream_at(p, model_id),
            model: found,
        })
    }

    /// FR-021. Never fails for a known provider.
    pub fn upstream_id(&self, provider: &str, model_id: &str) -> Result<String, NotFound> {
        self.provider_index(provider).map(|p| self.upstream_at(p, model_id))
    }

    pub fn unified_model(&self, name: &str) -> Result<&UnifiedModel, NotFound> {
        self.unified_index
            .get(name)
            .map(|&i| &self.unified[i])
            .ok_or_else(|| NotFound::UnifiedModel { name: name.to_owned() })
    }

    pub fn unified_models(&self) -> impl Iterator<Item = &UnifiedModel> {
        self.unified.iter()
    }

    /// Operator settings for `provider_id`; the default when `config.toml` has none.
    pub fn settings(&self, provider_id: &str) -> ProviderSettings {
        self.settings.get(provider_id).copied().unwrap_or_default()
    }

    /// Whether the bundled client secret for `provider_id` is released in this snapshot.
    pub fn credential(&self, provider_id: &str) -> Option<&ResolvedCredential> {
        self.credentials.get(provider_id)
    }

    pub fn report(&self) -> &LoadReport {
        &self.report
    }
}
