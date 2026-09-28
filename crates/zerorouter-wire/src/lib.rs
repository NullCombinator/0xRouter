//! The API-style interpreter (spec 003, research R3).
//!
//! A client API style is a data file. This crate reads it and translates between client
//! bodies, the intermediate representation in [`ir`], and provider wire bodies. It does no
//! I/O and has no async runtime, so translation is tested and benchmarked on its own.

pub mod codec;
pub mod error_body;
pub mod estimate;
pub mod ir;
pub mod primitives;
pub mod stream;
pub mod template;
pub mod usage;
