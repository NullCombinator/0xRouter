//! The adapter sandbox: a wasmtime host with a fresh instance per call.

pub mod abi;
pub mod call;
pub mod engine;
pub mod module;

pub use engine::{EngineError, SandboxEngine};
pub use module::{LoadError, LoadedModule, ModuleFlags, load, wasm_hash};
