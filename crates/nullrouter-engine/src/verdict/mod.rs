//! Verdicts: whether a model works on one account (spec 011, data-model.md § Verdict).
//!
//! A verdict belongs to a [`Pair`]: provider, account and upstream model id. The [`Board`] holds
//! every verdict in memory as one swappable map, so `plan` reads it without a lock, and writes
//! each change to `routing/verdicts.jsonl` through the journal writer. It lives on the engine,
//! not in the swapped snapshot, so a reload keeps it.

pub mod judge;
pub mod store;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use arc_swap::ArcSwap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::journal::{Journal, Target};

/// The account a no-auth provider's pairs use.
pub const NO_ACCOUNT: &str = "-";

/// Provider, account and upstream model id (`plan::Candidate::upstream_id`), so a unified-model
/// member and a direct target naming the same model share one pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pair {
    pub provider: String,
    pub account: String,
    pub model: String,
}

impl Pair {
    pub fn new(provider: impl Into<String>, account: impl Into<String>, model: impl Into<String>) -> Self {
        Self { provider: provider.into(), account: account.into(), model: model.into() }
    }
}

impl fmt::Display for Pair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{} {}", self.provider, self.account, self.model)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Pass,
    Broken,
    Unknown,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Broken => "broken",
            Self::Unknown => "unknown",
        }
    }

    /// `PASS`, `BROKEN`, `UNKNOWN`, as the CLI prints it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Broken => "BROKEN",
            Self::Unknown => "UNKNOWN",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pass" => Some(Self::Pass),
            "broken" => Some(Self::Broken),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Which definitive rejection made a pair BROKEN (research R3, R4).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Rejection {
    ModelNotFound,
    ModelNotAvailable,
    TypeNotSupported,
    /// A plugin's `[[rejections]]` rule matched; holds the rule's reason.
    Plugin(String),
}

impl Rejection {
    pub fn as_str(&self) -> String {
        match self {
            Self::ModelNotFound => "model_not_found".into(),
            Self::ModelNotAvailable => "model_not_available".into(),
            Self::TypeNotSupported => "type_not_supported".into(),
            Self::Plugin(r) => format!("plugin:{r}"),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "model_not_found" => Self::ModelNotFound,
            "model_not_available" => Self::ModelNotAvailable,
            "type_not_supported" => Self::TypeNotSupported,
            _ => Self::Plugin(s.strip_prefix("plugin:")?.to_owned()),
        })
    }

    /// The words the CLI prints before the provider's message; a plugin rule reads as its reason.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::ModelNotFound => "model not found",
            Self::ModelNotAvailable => "model not available",
            Self::TypeNotSupported => "type not supported",
            Self::Plugin(r) => match r.as_str() {
                "model_not_found" => "model not found",
                "model_not_available" => "model not available",
                "type_not_supported" => "type not supported",
                _ => "rejected",
            },
        }
    }
}

impl Serialize for Rejection {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for Rejection {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown rejection {s:?}")))
    }
}

/// What set a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Test,
    Retest,
    ComboTest,
    Operator,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Retest => "retest",
            Self::ComboTest => "combo_test",
            Self::Operator => "operator",
        }
    }
}

/// What a verdict was reached under (research R7). A change to any part resets it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Basis {
    /// `sha256:<hex>` of the install id and the account's key; key accounts only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// When the account last signed in; sign-in accounts only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_in_at: Option<String>,
    /// `sha256:<hex>` of the provider's plugin source.
    pub plugin: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub state: State,
    /// The provider's status and message (redacted, at most 500 characters), a timeout, a
    /// malformed answer, or the operator's note.
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection: Option<Rejection>,
    pub source: Source,
    #[serde(with = "time_serde")]
    pub at: SystemTime,
    /// The test call's record id; none for the operator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    /// UNKNOWN only: the retest step reached (0 = first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    /// When the next retest is due: UNKNOWN, and BROKEN from a test with BROKEN retests on.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_time_serde")]
    pub next: Option<SystemTime>,
    pub basis: Basis,
    /// The operator's note, from `verdicts mark --note`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A combo test's own result (data-model.md § Combo result). It never steers routing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComboVerdict {
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<String>,
    pub reason: String,
    #[serde(with = "time_serde")]
    pub at: SystemTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_time_serde")]
    pub next: Option<SystemTime>,
    /// `sha256:<hex>` of the combo's flattened member list; a change clears the result.
    pub definition: String,
}

/// Which verdicts `verdicts list` shows.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub provider: Option<String>,
    pub account: Option<String>,
    pub model: Option<String>,
    pub state: Option<State>,
}

impl Filter {
    fn keeps(&self, pair: &Pair, v: &Verdict) -> bool {
        self.provider.as_ref().is_none_or(|p| *p == pair.provider)
            && self.account.as_ref().is_none_or(|a| *a == pair.account)
            && self.model.as_ref().is_none_or(|m| *m == pair.model)
            && self.state.is_none_or(|s| s == v.state)
    }
}

type Models = HashMap<String, Verdict>;

/// Every verdict, keyed provider → account → model so a lookup by `&str` allocates nothing.
#[derive(Debug, Clone, Default)]
pub struct Verdicts {
    pairs: HashMap<String, HashMap<String, Models>>,
    len: usize,
    /// Combo results by combo name.
    pub combos: BTreeMap<String, ComboVerdict>,
}

impl Verdicts {
    pub fn get(&self, provider: &str, account: &str, model: &str) -> Option<&Verdict> {
        self.pairs.get(provider)?.get(account)?.get(model)
    }

    pub fn is_broken(&self, provider: &str, account: &str, model: &str) -> bool {
        self.get(provider, account, model).is_some_and(|v| v.state == State::Broken)
    }

    /// How many pairs have a verdict.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Every pair's verdict, sorted by pair.
    pub fn all(&self) -> Vec<(Pair, &Verdict)> {
        let mut out: Vec<(Pair, &Verdict)> = self
            .pairs
            .iter()
            .flat_map(|(p, accounts)| {
                accounts.iter().flat_map(move |(a, models)| models.iter().map(move |(m, v)| (Pair::new(p, a, m), v)))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub(crate) fn insert(&mut self, pair: Pair, v: Verdict) {
        let models = self.pairs.entry(pair.provider).or_default().entry(pair.account).or_default();
        if models.insert(pair.model, v).is_none() {
            self.len += 1;
        }
    }

    pub(crate) fn remove(&mut self, pair: &Pair) -> Option<Verdict> {
        let accounts = self.pairs.get_mut(&pair.provider)?;
        let models = accounts.get_mut(&pair.account)?;
        let gone = models.remove(&pair.model)?;
        if models.is_empty() {
            accounts.remove(&pair.account);
        }
        if accounts.is_empty() {
            self.pairs.remove(&pair.provider);
        }
        self.len -= 1;
        Some(gone)
    }
}

/// The live verdicts and their file. Writes are serialised, so the file's lines are in the order
/// the map changed.
pub struct Board {
    current: ArcSwap<Verdicts>,
    /// Lines in the file since it was last compacted.
    lines: Mutex<usize>,
    journal: Option<Arc<Journal>>,
}

impl fmt::Debug for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Board").field("pairs", &self.current.load().len()).finish()
    }
}

impl Default for Board {
    fn default() -> Self {
        Self::in_memory(Verdicts::default())
    }
}

impl Board {
    /// The board replayed from `routing/verdicts.jsonl` under `home`, writing through `journal`.
    pub fn open(home: &std::path::Path, journal: Arc<Journal>) -> (Self, store::Replay) {
        let replay = store::load(home);
        let board = Self {
            current: ArcSwap::from_pointee(replay.verdicts.clone()),
            lines: Mutex::new(replay.lines),
            journal: Some(journal),
        };
        (board, replay)
    }

    /// A board that keeps nothing on disk.
    pub fn in_memory(verdicts: Verdicts) -> Self {
        Self { current: ArcSwap::from_pointee(verdicts), lines: Mutex::new(0), journal: None }
    }

    /// The verdicts as they are now; cheap, lock-free.
    pub fn snapshot(&self) -> Arc<Verdicts> {
        self.current.load_full()
    }

    pub fn get(&self, pair: &Pair) -> Option<Verdict> {
        self.current.load().get(&pair.provider, &pair.account, &pair.model).cloned()
    }

    pub fn is_broken(&self, provider: &str, account: &str, model: &str) -> bool {
        self.current.load().is_broken(provider, account, model)
    }

    /// Every verdict `filter` keeps, sorted by pair.
    pub fn list(&self, filter: &Filter) -> Vec<(Pair, Verdict)> {
        let now = self.current.load();
        now.all().into_iter().filter(|(p, v)| filter.keeps(p, v)).map(|(p, v)| (p, v.clone())).collect()
    }

    /// Sets `pair`'s verdict and writes the line.
    pub fn set(&self, pair: Pair, v: Verdict) {
        let line = store::set_line(&pair, &v);
        self.change(store::VERDICT, line, |all| all.insert(pair, v));
    }

    /// Clears `pair`'s verdict (it is untested again). `false` when it had none.
    pub fn clear(&self, pair: &Pair, why: &str, at: SystemTime) -> bool {
        if self.get(pair).is_none() {
            return false;
        }
        let line = store::cleared_line(pair, why, at);
        self.change(store::VERDICT, line, |all| {
            all.remove(pair);
        });
        true
    }

    /// Clears every verdict `pick` names a reason for, each with its own `why`. Returns how many.
    pub fn clear_where(&self, at: SystemTime, mut pick: impl FnMut(&Pair, &Verdict) -> Option<&'static str>) -> usize {
        let gone: Vec<(Pair, &'static str)> =
            self.current.load().all().into_iter().filter_map(|(p, v)| Some((p.clone(), pick(&p, v)?))).collect();
        for (pair, why) in &gone {
            self.clear(pair, why, at);
        }
        gone.len()
    }

    pub fn combo(&self, name: &str) -> Option<ComboVerdict> {
        self.current.load().combos.get(name).cloned()
    }

    pub fn set_combo(&self, name: &str, v: ComboVerdict) {
        let line = store::combo_line(name, &v);
        self.change(store::COMBO, line, |all| {
            all.combos.insert(name.to_owned(), v);
        });
    }

    /// Clears a combo's result. `false` when it had none.
    pub fn clear_combo(&self, name: &str, why: &str, at: SystemTime) -> bool {
        if self.combo(name).is_none() {
            return false;
        }
        let line = store::combo_cleared_line(name, why, at);
        self.change(store::COMBO, line, |all| {
            all.combos.remove(name);
        });
        true
    }

    fn change(&self, t: &str, line: serde_json::Value, edit: impl FnOnce(&mut Verdicts)) {
        let mut lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = Verdicts::clone(&self.current.load());
        edit(&mut next);
        let live = next.len() + next.combos.len();
        self.current.store(Arc::new(next));
        let Some(journal) = &self.journal else { return };
        journal.append(Target::Verdicts, t, line);
        *lines += 1;
        // Compact once the file holds more than 4× the live entries, as `journal/state.rs` does.
        if *lines > 4 * live.max(16) {
            let body = store::render_all(&self.current.load());
            *lines = live;
            journal.replace(Target::Verdicts, body);
        }
    }
}

mod time_serde {
    use std::time::SystemTime;

    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::clock::rfc3339(*t))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let text = String::deserialize(d)?;
        crate::clock::parse_rfc3339(&text).ok_or_else(|| D::Error::custom(format!("{text:?} is not a time")))
    }
}

mod opt_time_serde {
    use std::time::SystemTime;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => super::time_serde::serialize(t, s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(text) => crate::clock::parse_rfc3339(&text)
                .map(Some)
                .ok_or_else(|| serde::de::Error::custom(format!("{text:?} is not a time"))),
            None => Ok(None),
        }
    }
}
