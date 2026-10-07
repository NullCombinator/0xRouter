//! `combos [NAME]`: the combos the registry loaded, and those it dropped (spec 011).
//!
//! `--json` without NAME: `{"combos":[<entry>…],"dropped":[{"name","reason"}]}`. An entry is
//! `{"kind":"combo","name","model_kind","members":[…]}`, where each member is
//! `{"name","kind":"unified"}` or a nested combo's entry, so the tree is whole. With NAME it is
//! that one entry, or `{"kind":"not_found","error"}`; the CLI exits 2 on it. `resolve NAME`
//! prints the same entry for a combo.

use nullrouter_registry::{Combo, OperatorHome, Registry};
use serde_json::{Value, json};

use super::{Live, View, ViewError, open_registry};

pub const NEEDS: &[&str] = &[];

/// What `combos NAME` and `resolve NAME` print for a combo.
pub fn combo_json(reg: &Registry, c: &Combo) -> Value {
    let members = c.members.iter().map(|m| match reg.combo(m) {
        Some(nested) => combo_json(reg, nested),
        None => json!({ "name": m, "kind": "unified" }),
    });
    json!({
        "kind": "combo", "name": c.name, "model_kind": c.kind.map(|k| k.as_str()),
        "members": members.collect::<Vec<_>>(),
    })
}

/// Arguments: `name`, or null for the list.
pub fn build(home: &OperatorHome, args: &Value, _live: &Live) -> Result<View, ViewError> {
    let reg = open_registry(home)?.snapshot();
    if let Some(name) = args["name"].as_str() {
        let json = match reg.combo(name) {
            Some(c) => combo_json(&reg, c),
            None => json!({ "kind": "not_found", "error": format!("no combo named {name:?}") }),
        };
        return Ok(View::new(json));
    }
    let dropped = reg
        .report()
        .dropped_combos
        .iter()
        .map(|d| json!({ "name": d.name, "reason": format!("needs unified model {} (dropped)", d.unified) }));
    let combos = reg.combos().map(|c| combo_json(&reg, c));
    Ok(View::new(json!({ "combos": combos.collect::<Vec<_>>(), "dropped": dropped.collect::<Vec<_>>() })))
}
