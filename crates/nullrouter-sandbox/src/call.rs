//! One call into a loaded adapter module: a fresh instance, a deadline, a memory ceiling.
//!
//! Nothing survives a call. The store, the instance and its memory are built for it and dropped
//! after, so one agent's content can't reach another's (research R3).

use std::time::{Duration, Instant};

use wasmtime::{ResourceLimiter, Store, Trap, UpdateDeadline};

use crate::abi::{EXPORT_ALLOC, EXPORT_MEMORY, MAX_IO, unpack};
use crate::engine::{MEMORY_LIMIT, SandboxEngine};
use crate::module::{LoadedModule, Redactor, State};

/// Longest a module's table may grow to.
const TABLE_LIMIT: usize = 10_000;

/// Which export a call enters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Request,
    Response,
    Event,
}

impl Entry {
    fn export(self) -> &'static str {
        match self {
            Entry::Request => "zr_on_request",
            Entry::Response => "zr_on_response",
            Entry::Event => "zr_on_event",
        }
    }
}

/// Why a call produced no output. None of these says anything about the adapter's honesty; only
/// the guardrail does (FR-016). Text here is fixed wording and wasmtime's trap codes, never the
/// request's content (FR-025).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallError {
    #[error("deadline")]
    Deadline,
    #[error("memory")]
    Memory,
    #[error("trap: {0}")]
    Trap(String),
    #[error("invalid output: {0}")]
    InvalidOutput(&'static str),
    #[error("input over the size limit")]
    InputTooLarge,
}

impl CallError {
    /// The `failed{…}` reason the record carries.
    pub fn code(&self) -> &'static str {
        match self {
            CallError::Deadline => "deadline",
            CallError::Memory => "memory",
            CallError::Trap(_) => "trap",
            CallError::InvalidOutput(_) | CallError::InputTooLarge => "invalid_output",
        }
    }
}

impl ResourceLimiter for State {
    fn memory_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        if desired > MEMORY_LIMIT {
            // An error, not `false`: a refusal the module could ignore would let it carry on.
            self.memory_exceeded = true;
            wasmtime::bail!("memory limit");
        }
        Ok(true)
    }

    fn table_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        Ok(desired <= TABLE_LIMIT)
    }

    fn instances(&self) -> usize {
        1
    }
}

/// What went wrong inside a call, before it is told apart.
enum Failure {
    Wasm(wasmtime::Error),
    Output(&'static str),
}

impl From<wasmtime::Error> for Failure {
    fn from(e: wasmtime::Error) -> Self {
        Failure::Wasm(e)
    }
}

/// Runs `entry` on a fresh instance of `module` with `input`, for at most `deadline`.
///
/// Returns the module's output bytes (not yet checked as JSON), or `None` when it made no edits.
/// The guest yields to the Tokio executor every millisecond, so a long call doesn't hold its
/// worker, and it is interrupted once `deadline` has passed.
pub async fn call(
    engine: &SandboxEngine,
    module: &LoadedModule,
    entry: Entry,
    input: &[u8],
    deadline: Duration,
    redact: Redactor,
) -> Result<Option<Vec<u8>>, CallError> {
    if input.len() > MAX_IO {
        return Err(CallError::InputTooLarge);
    }
    let end = Instant::now() + deadline;
    let mut store = Store::new(engine.engine(), State::new(redact, engine.enter()));
    store.limiter(|state| state);
    store.set_epoch_deadline(1);
    // Yielding with a new deadline alone would never stop a loop; this stops it at `end`.
    store.epoch_deadline_callback(move |_| {
        Ok(if Instant::now() >= end {
            UpdateDeadline::Interrupt
        } else {
            UpdateDeadline::YieldCustom(1, Box::pin(tokio::task::yield_now()))
        })
    });
    match run(&mut store, module, entry, input).await {
        Ok(out) => Ok(out),
        Err(Failure::Output(why)) => Err(CallError::InvalidOutput(why)),
        Err(Failure::Wasm(e)) => Err(classify(store.data(), &e)),
    }
}

async fn run(
    store: &mut Store<State>,
    module: &LoadedModule,
    entry: Entry,
    input: &[u8],
) -> Result<Option<Vec<u8>>, Failure> {
    let instance = module.pre.instantiate_async(&mut *store).await?;
    let memory = instance.get_memory(&mut *store, EXPORT_MEMORY).ok_or(Failure::Output("no memory"))?;
    let alloc = instance.get_typed_func::<i32, i32>(&mut *store, EXPORT_ALLOC)?;
    let on = instance.get_typed_func::<(i32, i32), i64>(&mut *store, entry.export())?;

    let len = i32::try_from(input.len()).map_err(|_| Failure::Output("input too large"))?;
    let ptr = alloc.call_async(&mut *store, len).await?;
    let start = ptr as u32 as usize;
    memory
        .data_mut(&mut *store)
        .get_mut(start..start + input.len())
        .ok_or(Failure::Output("allocation out of bounds"))?
        .copy_from_slice(input);

    let packed = on.call_async(&mut *store, (ptr, len)).await?;
    let Some((out_ptr, out_len)) = unpack(packed) else { return Ok(None) };
    if out_len > MAX_IO {
        return Err(Failure::Output("output over the size limit"));
    }
    let out = memory.data(&*store).get(out_ptr..out_ptr + out_len).ok_or(Failure::Output("output out of bounds"))?;
    Ok(Some(out.to_vec()))
}

fn classify(state: &State, e: &wasmtime::Error) -> CallError {
    if state.memory_exceeded {
        return CallError::Memory;
    }
    match e.downcast_ref::<Trap>() {
        Some(Trap::Interrupt) => CallError::Deadline,
        Some(trap) => CallError::Trap(trap.to_string()),
        None => CallError::Trap(e.to_string().lines().next().unwrap_or_default().to_owned()),
    }
}
