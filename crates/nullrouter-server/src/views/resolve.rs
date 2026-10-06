//! `resolve <target>`: what `provider/model` or a unified model name means. `member` and
//! `note_json` are shared with the `unified` view.
//!
//! `--json` shape (stable; the quickstart parses it):
//! `{"kind":"direct","provider","requested","upstream_id","catalogued"}`,
//! `{"kind":"unified","name","model_kind","members":[{"provider","requested","upstream_id","catalogued"}]}`,
//! or `{"kind":"not_found","error"}`. A unified answer also carries `"limits_notes":[{"unified","limit",
//! "values":[{"provider","value"}]}]`, empty when its members' limits agree. A target that isn't
//! found is an answer, not an error: the CLI exits 2.

use nullrouter_registry::{LimitsNote, OperatorHome, Registry, Resolution, UnifiedMember, UnifiedModel};
use serde_json::{Value, json};

use super::{Live, View, ViewError, open_registry};

pub const NEEDS: &[&str] = &[];

pub fn member(m: &UnifiedMember) -> Value {
    json!({ "provider": m.provider, "requested": m.requested, "upstream_id": m.upstream_id, "catalogued": m.catalogued })
}

pub fn note_json(n: &LimitsNote) -> Value {
    json!({ "unified": n.unified, "limit": n.limit,
            "values": n.values.iter().map(|(p, v)| json!({ "provider": p, "value": v })).collect::<Vec<_>>() })
}

/// What `resolve` prints for a unified model; the `unified` view lists the same objects.
pub fn unified_json(reg: &Registry, u: &UnifiedModel) -> Value {
    json!({
        "kind": "unified", "name": u.name, "model_kind": u.kind.map(|k| k.as_str()),
        "members": u.members.iter().map(member).collect::<Vec<_>>(),
        "limits_notes": reg.report().notes.iter().filter(|n| n.unified == u.name).map(note_json).collect::<Vec<_>>() })
}

/// Arguments: `target`. `extra.notes` is the text of the target's limits notes, which the text
/// shows as `note: …` lines and the JSON gives in structured form.
pub fn build(home: &OperatorHome, args: &Value, _live: &Live) -> Result<View, ViewError> {
    let target = args["target"].as_str().unwrap_or_default();
    let reg = open_registry(home)?.snapshot();
    let json = match reg.resolve(target) {
        Ok(Resolution::Direct { provider, requested, upstream_id, catalogued }) => json!({
            "kind": "direct", "provider": provider.id, "requested": requested,
            "upstream_id": upstream_id, "catalogued": catalogued }),
        Ok(Resolution::Unified(u)) => unified_json(&reg, u),
        Err(e) => json!({ "kind": "not_found", "error": e.to_string() }),
    };
    let notes: Vec<String> =
        reg.report().notes.iter().filter(|n| n.unified == target).map(ToString::to_string).collect();
    Ok(View { json, extra: json!({ "notes": notes }) })
}
