//! Request and response codecs driven by style files.

pub mod request;
pub mod response;
pub(crate) mod style;

pub use style::{EventTpl, PartTpl, Style, TextStyle};

/// A client body key a cross-style attempt couldn't carry (research R27). The value is
/// never kept: it may hold prompt text or a secret.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Dropped {
    pub path: String,
    pub reason: String,
}

/// Why a translation could not be done.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    /// The target wire has no place for this content. The target is skipped for this
    /// request; nothing is dropped (Constitution IV).
    #[error("the target can't carry {part}: {reason}")]
    CannotCarry { part: String, reason: String },
    /// The client body doesn't have the style's shape.
    #[error("{at}: {reason}")]
    Decode { at: String, reason: String },
    /// The style has no codec for this operation.
    #[error("style {style} has no {what}")]
    Missing { style: String, what: &'static str },
}

impl CodecError {
    pub(crate) fn carry(part: impl Into<String>, reason: impl Into<String>) -> Self {
        CodecError::CannotCarry { part: part.into(), reason: reason.into() }
    }

    pub(crate) fn decode(at: impl Into<String>, reason: impl Into<String>) -> Self {
        CodecError::Decode { at: at.into(), reason: reason.into() }
    }
}
