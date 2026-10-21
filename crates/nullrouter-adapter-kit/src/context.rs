//! The attempt context an adapter reads. It carries no secret, key, header, agent id, session
//! id or record id.

use serde::{Deserialize, Serialize};

/// The ABI major version this kit speaks.
pub const KIT_ABI: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Request,
    Response,
    Event,
}

/// Each `None` is unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub vision: Option<bool>,
    pub file_input: Option<bool>,
    pub reasoning: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    pub direction: Direction,
    pub provider: String,
    pub target_style: String,
    pub same_style: bool,
    pub model: String,
    pub model_type: String,
    pub capabilities: Capabilities,
    pub stream: bool,
    pub attempt: u32,
}
