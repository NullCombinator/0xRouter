//! The plugin validation gate.

mod errors;
pub(crate) mod gate;
mod secrets;
mod ssrf;
pub mod style_gate;

pub use errors::{FieldPath, Seg, ValidationError, line_col};
pub use gate::{GateCtx, Gated, validate, validate_with};
pub use secrets::{check_map_key, check_query, check_url, looks_like_secret};
pub use ssrf::{URL_PLACEHOLDERS, check_endpoint_url, is_private_ip};
pub use style_gate::{check_route_collisions, validate_style};
