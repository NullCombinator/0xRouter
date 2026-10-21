//! Loading an adapter module: the gate between bytes on disk and something that may run.
//!
//! A module is accepted only if its hash is the one the store recorded, it is a binary (never
//! text, never a precompiled artefact), its imports are exactly a subset of the kit's two host
//! functions, its required exports have the right types, and its ABI is one this core speaks.
//! Everything else is refused with the reason named.

use nullrouter_adapter_kit::KIT_ABI;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use wasmtime::{Caller, Extern, ExternType, InstancePre, Linker, Module, ValType};

use crate::engine::SandboxEngine;

/// The host functions a module may import, all from [`HOST_MODULE`].
pub const HOST_MODULE: &str = "nr";
const ALLOWED_IMPORTS: [&str; 2] = ["abi_version", "log"];
const ABI_SECTION: &str = "nr.abi";
const WASM_MAGIC: &[u8] = b"\0asm";

/// What the manifest says the module must export beyond `zr_on_request`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModuleFlags {
    /// The manifest has response selectors: `zr_on_response` is required.
    pub response: bool,
    /// `response.events = true`: `zr_on_event` is required.
    pub events: bool,
}

/// Removes secrets from a line before it is logged. The core passes its own redactor.
pub type Redactor = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Longest log line an adapter may write, and how many lines one call may write.
pub const MAX_LOG_BYTES: usize = 512;
pub const MAX_LOG_CALLS: u32 = 8;

/// What one call runs against: the limiter's verdict and the log budget.
pub struct State {
    pub(crate) memory_exceeded: bool,
    pub(crate) memory_limit: usize,
    logs: u32,
    redact: Redactor,
    /// Counts this call's store while it exists.
    _live: crate::engine::LiveGuard,
}

impl State {
    pub(crate) fn new(redact: Redactor, live: crate::engine::LiveGuard, memory_limit: usize) -> Self {
        Self { memory_exceeded: false, memory_limit, logs: 0, redact, _live: live }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("module hash is {found}, not the recorded {expected}")]
    HashMismatch { expected: String, found: String },
    #[error("not a WebAssembly binary")]
    NotBinary,
    #[error("module does not compile: {0}")]
    Invalid(String),
    #[error("module imports {module}.{name}")]
    ForbiddenImport { module: String, name: String },
    #[error("module has no `{0}` export")]
    MissingExport(&'static str),
    #[error("export `{0}` has the wrong type")]
    BadExport(&'static str),
    #[error("module has no `nr.abi` section")]
    AbiMissing,
    #[error("`nr.abi` section is not a 4-byte version")]
    AbiMalformed,
    #[error("module targets ABI {0}, which this core does not speak")]
    AbiUnsupported(u32),
    #[error("module does not link: {0}")]
    Link(String),
}

/// A module that passed the gate, ready to instantiate once per call.
pub struct LoadedModule {
    pub(crate) pre: InstancePre<State>,
    pub(crate) abi: u32,
    pub(crate) flags: ModuleFlags,
}

impl LoadedModule {
    pub fn abi(&self) -> u32 {
        self.abi
    }

    pub fn pre(&self) -> &InstancePre<State> {
        &self.pre
    }
}

impl std::fmt::Debug for LoadedModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedModule").field("abi", &self.abi).field("flags", &self.flags).finish_non_exhaustive()
    }
}

/// `sha256:<hex>` of `bytes`, the form the store records.
pub fn wasm_hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Whether this core speaks `abi`: the current major and the one before it.
fn supported(abi: u32) -> bool {
    abi == KIT_ABI || (KIT_ABI > 1 && abi == KIT_ABI - 1)
}

/// Runs every load check on `wasm`, which must hash to `expected_hash`.
pub fn load(
    engine: &SandboxEngine,
    wasm: &[u8],
    expected_hash: &str,
    flags: ModuleFlags,
) -> Result<LoadedModule, LoadError> {
    let found = wasm_hash(wasm);
    if found != expected_hash {
        return Err(LoadError::HashMismatch { expected: expected_hash.to_owned(), found });
    }
    // `Module::new` also reads text when a build enables wasmtime's `wat` feature, and only
    // `Module::new` is used here: a precompiled artefact is never deserialised.
    if !wasm.starts_with(WASM_MAGIC) {
        return Err(LoadError::NotBinary);
    }
    let abi = match custom_section(wasm, ABI_SECTION) {
        None => return Err(LoadError::AbiMissing),
        Some(data) => <[u8; 4]>::try_from(data).map(u32::from_le_bytes).map_err(|_| LoadError::AbiMalformed)?,
    };
    if !supported(abi) {
        return Err(LoadError::AbiUnsupported(abi));
    }

    let module = Module::new(engine.engine(), wasm).map_err(|e| LoadError::Invalid(e.to_string()))?;
    for import in module.imports() {
        if import.module() != HOST_MODULE || !ALLOWED_IMPORTS.contains(&import.name()) {
            return Err(LoadError::ForbiddenImport {
                module: import.module().to_owned(),
                name: import.name().to_owned(),
            });
        }
    }
    check_exports(&module, flags)?;

    let mut linker = Linker::<State>::new(engine.engine());
    link_host(&mut linker).map_err(|e| LoadError::Link(e.to_string()))?;
    let pre = linker.instantiate_pre(&module).map_err(|e| LoadError::Link(e.to_string()))?;
    Ok(LoadedModule { pre, abi, flags })
}

/// The two host functions. `log` writes at `debug`, at most [`MAX_LOG_BYTES`] bytes per line and
/// [`MAX_LOG_CALLS`] lines per call, redacted. A line past the budget, or one that points outside
/// the module's memory, is dropped without a trap: logging must not decide the call.
fn link_host(linker: &mut Linker<State>) -> wasmtime::Result<()> {
    linker.func_wrap(HOST_MODULE, "abi_version", || KIT_ABI as i32)?;
    linker.func_wrap(HOST_MODULE, "log", |mut caller: Caller<'_, State>, ptr: i32, len: i32| {
        let state = caller.data_mut();
        state.logs = state.logs.saturating_add(1);
        if state.logs > MAX_LOG_CALLS {
            return;
        }
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else { return };
        let (start, len) = (ptr as u32 as usize, (len as u32 as usize).min(MAX_LOG_BYTES));
        let Some(bytes) = start.checked_add(len).and_then(|end| memory.data(&caller).get(start..end)) else {
            return;
        };
        let line = String::from_utf8_lossy(bytes).into_owned();
        let line = (caller.data().redact)(&line);
        tracing::debug!(target: "nullrouter_sandbox::adapter", "{line}");
    })?;
    Ok(())
}

fn check_exports(module: &Module, flags: ModuleFlags) -> Result<(), LoadError> {
    use ValType::{I32, I64};
    match module.get_export("memory") {
        Some(ExternType::Memory(_)) => {}
        Some(_) => return Err(LoadError::BadExport("memory")),
        None => return Err(LoadError::MissingExport("memory")),
    }
    let mut wanted: Vec<(&'static str, &[ValType], &[ValType])> =
        vec![("zr_alloc", &[I32], &[I32]), ("zr_on_request", &[I32, I32], &[I64])];
    if flags.response {
        wanted.push(("zr_on_response", &[I32, I32], &[I64]));
    }
    if flags.events {
        wanted.push(("zr_on_event", &[I32, I32], &[I64]));
    }
    for (name, params, results) in wanted {
        match module.get_export(name) {
            None => return Err(LoadError::MissingExport(name)),
            Some(ty) if has_signature(&ty, params, results) => {}
            Some(_) => return Err(LoadError::BadExport(name)),
        }
    }
    Ok(())
}

fn has_signature(ty: &ExternType, params: &[ValType], results: &[ValType]) -> bool {
    let ExternType::Func(f) = ty else { return false };
    f.params().len() == params.len()
        && f.results().len() == results.len()
        && f.params().zip(params).all(|(a, b)| a.matches(b))
        && f.results().zip(results).all(|(a, b)| a.matches(b))
}

/// The payload of the first custom section called `wanted`, or `None` when there is none or the
/// binary's section framing is broken.
fn custom_section<'a>(wasm: &'a [u8], wanted: &str) -> Option<&'a [u8]> {
    let mut rest = wasm.get(8..)?;
    while let Some((&id, after_id)) = rest.split_first() {
        let (size, used) = leb_u32(after_id)?;
        let body = after_id.get(used..)?.get(..size)?;
        if id == 0 {
            let (name_len, used) = leb_u32(body)?;
            let name = body.get(used..)?.get(..name_len)?;
            if name == wanted.as_bytes() {
                return body.get(used + name_len..);
            }
        }
        rest = after_id.get(used + size..)?;
    }
    None
}

/// An unsigned LEB128 `u32` at the start of `bytes`: its value and how many bytes it took.
fn leb_u32(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut value: u64 = 0;
    for (i, &b) in bytes.iter().take(5).enumerate() {
        value |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Some((usize::try_from(value).ok()?, i + 1));
        }
    }
    None
}
