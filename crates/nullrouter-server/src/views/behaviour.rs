//! `behaviour show`: the operator's request-handling defaults, from `config.toml` alone.
//!
//! `--json`: `{"break_behaviour":{"value","default"}}`, one entry per `[pipeline]` setting, where
//! `default` says whether the value is the built-in one. A `config.toml` that doesn't load is the
//! startup error, exit 1, and no value.

use nullrouter_registry::schema::OperatorConfig;
use nullrouter_registry::{OperatorHome, load_config};
use serde_json::{Value, json};

use super::{Live, View, ViewError};

pub const NEEDS: &[&str] = &[];

pub fn build(home: &OperatorHome, _args: &Value, _live: &Live) -> Result<View, ViewError> {
    let config = load_config(home).map_err(|e| ViewError::failed(format!("startup failed:\n{e}")))?;
    let default = OperatorConfig::default();
    let value = config.pipeline.break_behaviour;
    Ok(View::new(json!({
        "break_behaviour": { "value": value.as_str(), "default": value == default.pipeline.break_behaviour },
    })))
}
