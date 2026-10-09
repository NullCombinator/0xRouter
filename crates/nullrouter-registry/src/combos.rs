//! Combos: ordered fallback chains of unified models or other combos, declared by the operator
//! as `[[combo]]` in `config.toml` (spec 011 research R12). Plugins cannot declare them.
//!
//! Combos load after unified models. A combo is checked for its name, its members, cycles and
//! its members' kinds; one that needs a unified model dropped at startup is dropped with it. A
//! loaded combo carries its walk: the unified models a request tries, depth-first, each once
//! (research R13), so a nested combo costs nothing extra at request time.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::registry::{Registry, UnifiedModel};
use crate::schema::{ComboDecl, ModelKind, OperatorConfig};
use crate::validate::FieldPath;

/// An operator-declared combo (data-model § Combo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    pub name: String,
    /// From its members; `None` when none is typed.
    pub kind: Option<ModelKind>,
    /// Unified model or combo names, in declaration order.
    pub members: Vec<String>,
    /// The unified models a request tries, depth-first, each once.
    pub flat: Vec<ComboStep>,
}

/// One unified model of a combo's walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComboStep {
    /// Index into the snapshot's unified models.
    pub(crate) unified: usize,
    /// The combos it was reached through, then its name: `coder › fallback-chain › gpt`.
    pub path: String,
}

/// A combo dropped at startup because a unified model it needs was dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedCombo {
    pub name: String,
    /// The dropped unified model.
    pub unified: String,
}

impl fmt::Display for DroppedCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dropped combo {}: needs unified model {} (dropped)", self.name, self.unified)
    }
}

pub(crate) struct ComboOutcome {
    pub(crate) combos: Vec<Combo>,
    pub(crate) dropped: Vec<DroppedCombo>,
    pub(crate) errors: Vec<(FieldPath, String)>,
}

#[derive(Debug, Clone, Copy)]
enum Member {
    Unified(usize),
    /// A unified model declared in `config.toml` but not loaded.
    Dropped,
    Combo(usize),
}

/// Checks `config.combo` against the loaded `unified` models and builds the combos.
pub(crate) fn load(config: &OperatorConfig, reg: &Registry, unified: &[UnifiedModel]) -> ComboOutcome {
    let decls = &config.combo;
    let mut errors = Vec::new();
    let unified_ix: HashMap<&str, usize> = unified.iter().enumerate().map(|(i, u)| (u.name.as_str(), i)).collect();
    let declared: HashSet<&str> = config.unified_model.iter().map(|u| u.name.as_str()).collect();

    let mut names: HashMap<&str, usize> = HashMap::new();
    for (i, d) in decls.iter().enumerate() {
        let at = FieldPath::of("combo").index(i).key("name");
        if d.name.is_empty() || d.name.contains('/') {
            errors.push((at, "must not be empty or contain \"/\"".into()));
        } else if declared.contains(d.name.as_str()) {
            errors.push((at, format!("name {:?} is already a unified model", d.name)));
        } else if let Some(&j) = names.get(d.name.as_str()) {
            errors.push((at, format!("name {:?} is already a combo (combo[{j}])", d.name)));
        } else {
            names.insert(&d.name, i);
        }
    }

    let mut edges: Vec<Vec<(Member, &str)>> = Vec::with_capacity(decls.len());
    for (i, d) in decls.iter().enumerate() {
        let at = FieldPath::of("combo").index(i).key("members");
        if d.members.is_empty() {
            errors.push((at.clone(), "must not be empty".into()));
        }
        let mut members = Vec::with_capacity(d.members.len());
        for (k, m) in d.members.iter().enumerate() {
            let member = if let Some(&j) = names.get(m.as_str()) {
                Member::Combo(j)
            } else if let Some(&u) = unified_ix.get(m.as_str()) {
                Member::Unified(u)
            } else if declared.contains(m.as_str()) {
                Member::Dropped
            } else {
                errors.push((at.index(k), format!("unknown unified model or combo {m:?}")));
                continue;
            };
            members.push((member, m.as_str()));
        }
        edges.push(members);
    }

    for cycle in cycles(&edges) {
        let path = cycle.iter().map(|&i| decls[i].name.as_str()).collect::<Vec<_>>().join(" → ");
        errors.push((FieldPath::of("combo").index(cycle[0]), format!("contains itself: {path}")));
    }
    if !errors.is_empty() {
        return ComboOutcome { combos: Vec::new(), dropped: Vec::new(), errors };
    }

    let mut g = Graph { decls, edges, reg, unified, kinds: vec![None; decls.len()], errors };
    let mut combos = Vec::new();
    let mut dropped = Vec::new();
    for (i, d) in decls.iter().enumerate() {
        let kind = g.kind(i);
        if let Some(u) = g.needs(i) {
            dropped.push(DroppedCombo { name: d.name.clone(), unified: u.to_owned() });
            continue;
        }
        let mut flat = Vec::new();
        g.flatten(i, "", &mut HashSet::new(), &mut flat);
        combos.push(Combo { name: d.name.clone(), kind, members: d.members.clone(), flat });
    }
    ComboOutcome { combos, dropped, errors: g.errors }
}

/// Every cycle among combos, each once, as the combos on it from where it was found back to
/// that combo.
fn cycles(edges: &[Vec<(Member, &str)>]) -> Vec<Vec<usize>> {
    type Edges<'e> = [Vec<(Member, &'e str)>];
    fn visit(i: usize, edges: &Edges<'_>, color: &mut [u8], stack: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        color[i] = 1;
        stack.push(i);
        for &(m, _) in &edges[i] {
            let Member::Combo(j) = m else { continue };
            match color[j] {
                0 => visit(j, edges, color, stack, out),
                1 => {
                    let from = stack.iter().position(|&s| s == j).unwrap_or(0);
                    let mut cycle = stack[from..].to_vec();
                    cycle.push(j);
                    out.push(cycle);
                }
                _ => {}
            }
        }
        stack.pop();
        color[i] = 2;
    }
    let mut color = vec![0u8; edges.len()];
    let mut out = Vec::new();
    for i in 0..edges.len() {
        if color[i] == 0 {
            visit(i, edges, &mut color, &mut Vec::new(), &mut out);
        }
    }
    out
}

/// The combos once names, members and cycles have passed.
struct Graph<'a> {
    decls: &'a [ComboDecl],
    edges: Vec<Vec<(Member, &'a str)>>,
    reg: &'a Registry,
    unified: &'a [UnifiedModel],
    /// Each combo's kind once worked out.
    kinds: Vec<Option<Option<ModelKind>>>,
    errors: Vec<(FieldPath, String)>,
}

impl Graph<'_> {
    /// Combo `i`'s kind: its typed members' kind. Members of two kinds are an error, reported
    /// once; untyped members never conflict.
    fn kind(&mut self, i: usize) -> Option<ModelKind> {
        if let Some(k) = self.kinds[i] {
            return k;
        }
        let mut first: Option<(ModelKind, &str)> = None;
        for (m, name) in self.edges[i].clone() {
            let kind = match m {
                Member::Unified(u) => unified_kind(self.reg, &self.unified[u]),
                Member::Combo(j) => self.kind(j),
                Member::Dropped => None,
            };
            let Some(kind) = kind else { continue };
            match first {
                None => first = Some((kind, name)),
                Some((want, _)) if want == kind => {}
                Some((want, who)) => {
                    let at = FieldPath::of("combo").index(i).key("members");
                    self.errors.push((at, format!("members disagree on kind: {who} is {want}, {name} is {kind}")));
                    break;
                }
            }
        }
        let kind = first.map(|(k, _)| k);
        self.kinds[i] = Some(kind);
        kind
    }

    /// The first dropped unified model combo `i` needs, through nested combos too.
    fn needs(&self, i: usize) -> Option<&str> {
        self.edges[i].iter().find_map(|&(m, name)| match m {
            Member::Dropped => Some(name),
            Member::Combo(j) => self.needs(j),
            Member::Unified(_) => None,
        })
    }

    /// Combo `i`'s unified models under `prefix`, depth-first, skipping any already `seen`.
    fn flatten(&self, i: usize, prefix: &str, seen: &mut HashSet<usize>, out: &mut Vec<ComboStep>) {
        let name = &self.decls[i].name;
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix} › {name}") };
        for &(m, member) in &self.edges[i] {
            match m {
                Member::Unified(u) if seen.insert(u) => {
                    out.push(ComboStep { unified: u, path: format!("{path} › {member}") });
                }
                Member::Combo(j) => self.flatten(j, &path, seen, out),
                _ => {}
            }
        }
    }
}

/// A unified model's kind: declared, else its first typed member's.
fn unified_kind(reg: &Registry, u: &UnifiedModel) -> Option<ModelKind> {
    u.kind.or_else(|| u.members.iter().find_map(|m| reg.model(&m.provider, &m.requested).ok()?.kind))
}
