//! The process-wide wasmtime engine and the ticker that drives its epoch.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
}

impl SandboxEngine {
    /// Builds the engine and starts the 1 ms epoch ticker, so it must be called inside a Tokio
    /// runtime. `max_instances` sizes the pooling allocator.
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
        tokio::spawn(async move {
            let mut every = tokio::time::interval(TICK);
            // A late tick is skipped, not replayed: a burst of epochs would trap a healthy call.
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            while !halt.load(Ordering::Relaxed) {
                every.tick().await;
                ticking.increment_epoch();
            }
        });
        Ok(Self { engine, stop })
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

impl Drop for SandboxEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
