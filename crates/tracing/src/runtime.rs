//! Runtime log level control for dynamic tracing configuration.
//!
//! This module provides the ability to dynamically change log levels at runtime,
//! particularly useful for debugging specific blocks during `engine_newPayload` processing.

use parking_lot::Mutex;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    OnceLock,
};
use tracing_subscriber::{reload, EnvFilter, Registry};

/// Control handle for dynamically reloading the stdout filter.
pub struct StdoutReloadControl {
    handle: reload::Handle<EnvFilter, Registry>,
    /// Stack of previous filters for nested overrides.
    stack: Mutex<Vec<EnvFilter>>,
    /// Base filter to restore when stack is empty.
    base_filter_str: String,
}

impl std::fmt::Debug for StdoutReloadControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdoutReloadControl")
            .field("base_filter_str", &self.base_filter_str)
            .finish_non_exhaustive()
    }
}

/// Global stdout reload control, initialized during tracer setup.
static STDOUT_RELOAD: OnceLock<StdoutReloadControl> = OnceLock::new();

/// Block number to trace with elevated logging. `u64::MAX` means disabled.
static TRACE_BLOCK_NUM: AtomicU64 = AtomicU64::new(u64::MAX);

/// Installs the stdout reload handle for runtime log level control.
///
/// This should be called once during tracer initialization.
pub fn install_stdout_reload(handle: reload::Handle<EnvFilter, Registry>, base_filter_str: String) {
    let _ = STDOUT_RELOAD.set(StdoutReloadControl {
        handle,
        stack: Mutex::new(Vec::new()),
        base_filter_str,
    });
}

/// Sets the block number for which trace-level logging should be enabled during `newPayload`.
///
/// Pass `None` to disable block-specific tracing.
pub fn set_trace_block(block: Option<u64>) {
    TRACE_BLOCK_NUM.store(block.unwrap_or(u64::MAX), Ordering::Relaxed);
}

/// Returns the block number for which trace-level logging is enabled, if any.
pub fn trace_block() -> Option<u64> {
    let v = TRACE_BLOCK_NUM.load(Ordering::Relaxed);
    (v != u64::MAX).then_some(v)
}

/// RAII guard that restores the previous log filter when dropped.
#[must_use]
#[derive(Debug)]
pub struct FilterOverrideGuard {
    _private: (),
}

impl Drop for FilterOverrideGuard {
    fn drop(&mut self) {
        if let Some(ctrl) = STDOUT_RELOAD.get() {
            let mut stack = ctrl.stack.lock();
            let prev = stack.pop().unwrap_or_else(|| {
                EnvFilter::try_new(&ctrl.base_filter_str).unwrap_or_else(|_| EnvFilter::new("info"))
            });
            let _ = ctrl.handle.reload(prev);
        }
    }
}

/// Pushes a new stdout filter, returning a guard that restores the previous filter on drop.
///
/// Returns `None` if the stdout reload control hasn't been installed.
pub fn push_stdout_filter(new_filter: EnvFilter) -> Option<FilterOverrideGuard> {
    let ctrl = STDOUT_RELOAD.get()?;
    let mut stack = ctrl.stack.lock();

    let prev = stack.last().cloned().unwrap_or_else(|| {
        EnvFilter::try_new(&ctrl.base_filter_str).unwrap_or_else(|_| EnvFilter::new("info"))
    });
    stack.push(prev);

    let _ = ctrl.handle.reload(new_filter);
    Some(FilterOverrideGuard { _private: () })
}

/// Enables trace-level logging for the given block if it matches the configured trace block.
///
/// Returns a guard that restores the previous log level when dropped.
/// The trace block setting is cleared after being triggered (one-shot behavior).
pub fn maybe_trace_newpayload_block(block_num: u64) -> Option<FilterOverrideGuard> {
    let target = trace_block()?;
    if target != block_num {
        return None;
    }

    set_trace_block(None);

    let filter = EnvFilter::new(
        "engine::tree=trace,\
         reth_blockchain_tree=trace,\
         reth_engine_tree=trace,\
         reth_payload_builder=debug,\
         reth_evm=debug,\
         reth_revm=debug,\
         reth_trie=debug,\
         info",
    );
    push_stdout_filter(filter)
}
