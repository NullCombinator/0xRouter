//! The process-wide wasmtime engine and the ticker that drives its epoch.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use wasmtime::{Config, Engine, InstanceAllocationStrategy, PoolingAllocationConfig};

/// Linear memory an adapter may use, in bytes (contracts/adapter-kit.md, Limits).
pub const MEMORY_LIMIT: usize = 64 << 20;

/// One epoch is one millisecond, so a deadline of N ms is N ticks.
const TICK: Duration = Duration::from_millis(1);

#[derive(Debug, thiserror::Error)]
#[error("sandbox engine: {0}")]
pub struct EngineError(String);

/// The engine every adapter module is compiled and run on. Dropping it stops the ticker.
#[derive(Debug)]
pub struct SandboxEngine {
    engine: Engine,
    stop: Arc<AtomicBool>,
    /// Calls whose store exists right now; see [`SandboxEngine::live_instances`].
    live: Arc<AtomicUsize>,
}

/// Counts one call's store in [`SandboxEngine::live_instances`] for as long as it lives. It sits
/// in the store's state, so a call future dropped mid-run gives its count back with the store.
#[derive(Debug)]
pub(crate) struct LiveGuard(Arc<AtomicUsize>);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl SandboxEngine {
    /// Builds the engine and starts the 1 ms epoch ticker. `max_instances` sizes the pooling
    /// allocator.
    ///
    /// The ticker is an OS thread, not a Tokio task: a guest runs on the thread that polls it, so
    /// a ticker sharing that thread could never advance the epoch while the guest loops.
    ///
    /// Async execution needs no setting: in wasmtime 45 `Config::async_support` is a deprecated
    /// no-op, and the `async` cargo feature is what enables it. Threads need none either: the
    /// workspace builds wasmtime without its `threads` feature, so the proposal isn't compiled in
    /// and `Config::wasm_threads` doesn't exist.
    pub fn new(max_instances: u32) -> Result<Self, EngineError> {
        let mut config = Config::new();
        config.epoch_interruption(true).consume_fuel(false).wasm_relaxed_simd(false).wasm_multi_memory(false);
        let mut pool = PoolingAllocationConfig::default();
        pool.total_core_instances(max_instances)
            .total_memories(max_instances)
            .total_tables(max_instances)
            .total_stacks(max_instances)
            .max_memory_size(MEMORY_LIMIT);
        config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
        let engine = Engine::new(&config).map_err(|e| EngineError(e.to_string()))?;

        let stop = Arc::new(AtomicBool::new(false));
        let (ticking, halt) = (engine.clone(), stop.clone());
        std::thread::Builder::new()
            .name("nr-epoch".into())
            .spawn(move || {
                while !halt.load(Ordering::Relaxed) {
                    std::thread::sleep(TICK);
                    ticking.increment_epoch();
                }
            })
            .map_err(|e| EngineError(e.to_string()))?;
        Ok(Self { engine, stop, live: Arc::new(AtomicUsize::new(0)) })
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// How many calls hold a store right now. It returns to 0 when every call has finished or
    /// been dropped: nothing outlives its call.
    pub fn live_instances(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    pub(crate) fn enter(&self) -> LiveGuard {
        self.live.fetch_add(1, Ordering::SeqCst);
        LiveGuard(self.live.clone())
    }
}

impl Drop for SandboxEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
