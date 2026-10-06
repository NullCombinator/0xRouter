//! The record journal and routing state files (spec 006, contracts/record-journal.md).
//!
//! A dedicated writer thread owns every file under `records/` and `routing/`. Request tasks only
//! send lines to it and await acks, so file I/O never blocks the async executor.

mod index;
pub mod records;
pub mod state;
pub mod writer;

pub use writer::{Ack, Health, JournalError, Options, Target, Writer as Journal};
