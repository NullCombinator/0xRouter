//! The adapter sandbox: a wasmtime host with a fresh instance per call.

pub mod abi;
pub mod call;
pub mod engine;
pub mod module;

pub use call::{CallError, Entry, call};
pub use engine::{EngineError, SandboxEngine};
pub use module::{LoadError, LoadedModule, ModuleFlags, Redactor, load, wasm_hash};
