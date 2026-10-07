//! `verdicts`: every model verdict per account, and the combo results (spec 011, contracts/cli.md
//! § `nullrouter verdicts`).
//!
//! `--json`: `{"verdicts":[{provider,account,model,state,reason,source,at,…,"waiting"?}],
//! "combos":[{combo,state,…}]}`, sorted by pair. With a server it is the `verdicts.list` answer,
//! which knows why a due retest waits; without one, `routing/verdicts.jsonl` as the server
//! would replay it, with no `waiting`.

use nullrouter_engine::verdict::{Board, Filter, State, store};
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};

use super::{Live, View, ViewError};

pub const NEEDS: &[&str] = &["verdicts.list"];

/// Arguments: `provider`, `account`, `model`, `state`, each optional.
pub fn build(home: &OperatorHome, args: &Value, live: &Live) -> Result<View, ViewError> {
    if let Some(a) = live.ok("verdicts.list")? {
        return Ok(View::new(json!({ "verdicts": a["verdicts"], "combos": a["combos"] })));
    }
    let str_of = |k: &str| args[k].as_str().map(str::to_owned);
    let state = match args["state"].as_str() {
        Some(s) => Some(State::parse(s).ok_or_else(|| ViewError::failed("--state is pass, broken or unknown"))?),
        None => None,
    };
    let filter = Filter { provider: str_of("provider"), account: str_of("account"), model: str_of("model"), state };
    let replay = store::load(home.path());
    let verdicts: Vec<Value> = Board::in_memory(replay.verdicts.clone())
        .list(&filter)
        .iter()
        .map(|(pair, v)| {
            let mut line = store::set_line(pair, v);
            if let Some(map) = line.as_object_mut() {
                map.remove("basis");
            }
            line
        })
        .collect();
    let by_pair = filter.provider.is_some() || filter.account.is_some() || filter.model.is_some();
    let combos: Vec<Value> = replay
        .verdicts
        .combos
        .iter()
        .filter(|(_, c)| !by_pair && state.is_none_or(|s| s == c.state))
        .map(|(name, c)| store::combo_line(name, c))
        .collect();
    Ok(View::new(json!({ "verdicts": verdicts, "combos": combos })))
}
