//! The quota fit (spec 012): learns each polled account's real meter from its poll history,
//! keeps outside use out of the evidence, and hands routing a precomputed meter in effect.
//!
//! The fit sits beside routing, not inside it (research R1). Rows come from the history that
//! already exists (R2), the model and its noise are R3 and R4, the one always-valid test is R5,
//! and classification, separability, split-off, breaks, alerts and epochs are R6–R12.

pub mod breaks;
pub mod classify;
pub mod learner;
pub mod linalg;
pub mod model;
pub mod outside;
pub mod rows;
pub mod split;
pub mod store;
pub mod test;

use std::fmt;
use std::str::FromStr;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::SystemTime;

use arc_swap::ArcSwap;
use indexmap::IndexMap;
use nullrouter_registry::schema::MeterDecl;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One of the four token classes a `weighted_tokens` window charges differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenClass {
    Input,
    Output,
    CacheRead,
    CacheWrite,
}

impl TokenClass {
    pub const ALL: [TokenClass; 4] = [Self::Input, Self::Output, Self::CacheRead, Self::CacheWrite];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::CacheRead => "cache_read",
            Self::CacheWrite => "cache_write",
        }
    }
}

impl FromStr for TokenClass {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Self::ALL.into_iter().find(|c| c.as_str() == s).ok_or_else(|| format!("unknown token class \"{s}\""))
    }
}

/// A name for one number of one window's meter (data-model § Meter number). `Capacity` is per
/// account; weights and multipliers are pooled per plugin.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MeterNumber {
    Capacity,
    Weight(TokenClass),
    /// The glob as the plugin declares it.
    Multiplier(String),
}

impl fmt::Display for MeterNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capacity => f.write_str("capacity"),
            Self::Weight(c) => write!(f, "weight.{}", c.as_str()),
            Self::Multiplier(g) => write!(f, "multiplier.{g}"),
        }
    }
}

impl FromStr for MeterNumber {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        if s == "capacity" {
            Ok(Self::Capacity)
        } else if let Some(c) = s.strip_prefix("weight.") {
            c.parse().map(Self::Weight)
        } else if let Some(g) = s.strip_prefix("multiplier.").filter(|g| !g.is_empty()) {
            Ok(Self::Multiplier(g.to_owned()))
        } else {
            Err(format!("unknown meter number \"{s}\""))
        }
    }
}

impl Serialize for MeterNumber {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MeterNumber {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Where the value in effect for a number came from (FR-012), strongest first. A plugin
/// override never applies to `Capacity` (clarify Q5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    AccountOverride,
    PluginOverride,
    Fit,
    Declared,
}

/// How far a number's fit has come (FR-028).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// Poll intervals that inform the number.
    pub intervals: u64,
    /// Half the 95% range, as a fraction of the estimate.
    pub half_width: f64,
}

/// Where a number is in its life (data-model § Number state).
#[derive(Debug, Clone, PartialEq)]
pub enum NumberState {
    Learning { progress: Progress },
    Fitted { since: SystemTime },
    /// Learning again after a break at `since`.
    Relearning { since: SystemTime, progress: Progress },
    /// Learning again after the plugin's meter changed.
    Restarted { since: SystemTime, reason: String },
    /// The traffic can't tell it from `partner`; it stays declared.
    NotSeparable { partner: String },
    /// `weight.input` on a percent window: held, never fitted (clarify Q1).
    Yardstick,
    /// A window that reports no limit has no capacity to fit.
    NotReported,
    /// An account with no quota reports, or pay-as-you-go.
    NotFitted(String),
}

/// Numbers an operator set by hand, at one level (an account, or every account of a plugin).
/// Capacity only applies at account level.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NumberOverrides {
    pub capacity: Option<f64>,
    /// Indexed like `TokenClass::ALL`.
    pub weights: [Option<f64>; 4],
    /// Glob → factor; globs the plugin doesn't declare are ignored.
    pub multipliers: IndexMap<String, f64>,
}

/// The significant fitted numbers of one window; numbers not here are not significant.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowFit {
    /// Fitted weights. On a percent window they are ratios to the input weight (`ρ`), scaled by
    /// the input weight in effect when applied; `weight.input` itself is never fitted there.
    pub weights: BTreeMap<TokenClass, f64>,
    pub relative_to_input: bool,
    pub multipliers: BTreeMap<String, f64>,
    /// Per account, in the window's unit.
    pub capacity: BTreeMap<String, f64>,
}

/// The meter in effect for one account's window, and where each non-declared number came from
/// (FR-012). Precedence per number: account override, plugin override (never capacity), fitted
/// value, declaration. With nothing to apply, the result equals `declared` field for field
/// (FR-011), and the source map is empty.
pub fn in_effect(
    declared: &MeterDecl,
    plugin: Option<&NumberOverrides>,
    account: Option<&NumberOverrides>,
    fit: &WindowFit,
    account_name: &str,
) -> (MeterDecl, BTreeMap<MeterNumber, Source>) {
    let mut meter = declared.clone();
    let mut sources = BTreeMap::new();

    // Capacity: account override, fit, declaration.
    if let Some(v) = account.and_then(|o| o.capacity) {
        meter.capacity = Some(v);
        sources.insert(MeterNumber::Capacity, Source::AccountOverride);
    } else if let Some(v) = fit.capacity.get(account_name) {
        meter.capacity = Some(*v);
        sources.insert(MeterNumber::Capacity, Source::Fit);
    }

    // Weights. A percent window's input weight is the yardstick: only an override moves it.
    let base = declared.token_weights.unwrap_or_default();
    let declared_of = |c: TokenClass| match c {
        TokenClass::Input => base.input,
        TokenClass::Output => base.output,
        TokenClass::CacheRead => base.cache_read,
        TokenClass::CacheWrite => base.cache_write,
    };
    let index = |c: TokenClass| TokenClass::ALL.iter().position(|x| *x == c).unwrap_or(0);
    let pick = |o: Option<&NumberOverrides>, c: TokenClass| o.and_then(|o| o.weights[index(c)]);
    let input = pick(account, TokenClass::Input).or(pick(plugin, TokenClass::Input)).unwrap_or(base.input);
    let mut weights = base;
    let mut changed = false;
    for c in TokenClass::ALL {
        let (value, source) = if let Some(v) = pick(account, c) {
            (v, Source::AccountOverride)
        } else if let Some(v) = pick(plugin, c) {
            (v, Source::PluginOverride)
        } else if let Some(v) = fit.weights.get(&c).filter(|_| !(fit.relative_to_input && c == TokenClass::Input)) {
            (if fit.relative_to_input { v * input } else { *v }, Source::Fit)
        } else {
            (declared_of(c), Source::Declared)
        };
        if source == Source::Declared {
            // The input weight can still have moved as the yardstick of fitted ratios.
            continue;
        }
        match c {
            TokenClass::Input => weights.input = value,
            TokenClass::Output => weights.output = value,
            TokenClass::CacheRead => weights.cache_read = value,
            TokenClass::CacheWrite => weights.cache_write = value,
        }
        sources.insert(MeterNumber::Weight(c), source);
        changed = true;
    }
    if changed {
        meter.token_weights = Some(weights);
    }

    // Multipliers keep the plugin's glob order.
    for (glob, value) in meter.model_multiplier.iter_mut() {
        let pick = |o: Option<&NumberOverrides>| o.and_then(|o| o.multipliers.get(glob)).copied();
        let (v, source) = if let Some(v) = pick(account) {
            (v, Source::AccountOverride)
        } else if let Some(v) = pick(plugin) {
            (v, Source::PluginOverride)
        } else if let Some(v) = fit.multipliers.get(glob) {
            (*v, Source::Fit)
        } else {
            continue;
        };
        *value = v;
        sources.insert(MeterNumber::Multiplier(glob.clone()), source);
    }
    (meter, sources)
}

/// The fits of every plugin window, keyed by provider, then window name.
#[derive(Debug, Clone, Default)]
pub struct Fits {
    pub windows: BTreeMap<String, BTreeMap<String, WindowFit>>,
}

impl Fits {
    /// The significant numbers of one window; empty when none (or the window is unknown).
    pub fn window(&self, provider: &str, window: &str) -> WindowFit {
        self.windows.get(provider).and_then(|w| w.get(window)).cloned().unwrap_or_default()
    }
}

/// One account's meters in effect: its provider's declared windows with the numbers the fit and
/// the overrides replaced, and where each replaced number came from, by window name.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountMeters {
    pub windows: Arc<[MeterDecl]>,
    pub sources: BTreeMap<String, BTreeMap<MeterNumber, Source>>,
}

/// The meters in effect of every account that has any replaced number, swapped whole after each
/// change (research R15). An account with none isn't here: routing reads its provider's
/// declaration, so placements equal today's by construction (FR-011).
#[derive(Debug, Default)]
pub struct Meters {
    cells: ArcSwap<HashMap<(String, String), Arc<AccountMeters>>>,
}

impl Meters {
    /// One load per candidate on the request path.
    pub fn get(&self, provider: &str, account: &str) -> Option<Arc<AccountMeters>> {
        self.cells.load().get(&(provider.to_owned(), account.to_owned())).cloned()
    }

    /// Recomputes every account's meters from the registry in `st` and the fits.
    pub fn rebuild(&self, st: &crate::state::EngineState, fits: &Fits) {
        let mut next = HashMap::new();
        for account in st.accounts.iter() {
            let Some(provider) = st.registry.providers().find(|p| p.id == account.provider) else { continue };
            let declared = provider.routing().windows;
            let mut windows = Vec::with_capacity(declared.len());
            let mut sources = BTreeMap::new();
            for meter in declared {
                let fit = fits.window(&account.provider, &meter.name);
                let (m, src) = in_effect(meter, None, None, &fit, &account.name);
                if !src.is_empty() {
                    sources.insert(meter.name.clone(), src);
                }
                windows.push(m);
            }
            if !sources.is_empty() {
                next.insert((account.provider.clone(), account.name.clone()), Arc::new(AccountMeters { windows: windows.into(), sources }));
            }
        }
        self.cells.store(Arc::new(next));
    }
}
