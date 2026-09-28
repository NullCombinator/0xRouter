//! The 0router HTTP surface (spec 003): routes built from the loaded API styles, the
//! access-key check, the streaming relay, model lists, token counts, and the operator
//! socket. A thin layer over `zerorouter-engine`.

pub mod auth;
pub mod count;
pub mod models;
pub mod operator;
pub mod relay;
pub mod router;
pub mod serve;
pub mod text;
