//! The intermediate representation every style translates through (research R3).
//!
//! Content is carried, never rewritten (Constitution IV): a decoder keeps every part it
//! reads, and an encoder that can't place a part refuses the target instead of dropping it.

mod event;
mod request;
mod response;

pub use event::{BlockKind, ErrorEvent, Event};
pub use nullrouter_registry::schema::FinishReason;
pub use request::{
    EmbeddingsRequest, ImageRequest, Media, MediaSource, Message, Opaque, Params, Part, Request, ResponseFormat,
    ResultContent, Role, SttRequest, Thinking, Tool, ToolChoice, TtsRequest, VideoRequest,
};
pub use response::{Response, Usage};
