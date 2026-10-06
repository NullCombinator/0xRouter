//! The 0router HTTP surface (spec 003): routes built from the loaded API styles, the
//! access-key check, the streaming relay, model lists, token counts, and the operator
//! socket. A thin layer over `nullrouter-engine`.

pub mod auth;
pub mod count;
pub mod media;
pub mod models;
pub mod operator;
pub mod quota;
pub mod relay;
pub mod router;
pub mod serve;
pub mod text;
pub mod views;
