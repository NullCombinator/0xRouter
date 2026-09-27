//! The plugin validation gate.

mod errors;
pub(crate) mod gate;
mod secrets;

pub use errors::{FieldPath, Seg, ValidationError, line_col};
pub use gate::validate;
pub use secrets::{check_map_key, check_query, check_url};
