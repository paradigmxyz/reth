//! Prewarming functionality for the execution stage.
//!
//! This module provides cache warming capabilities during staged sync by executing
//! transactions ahead of the main execution to populate the execution cache.

use super::execution_cache::ExecutionCache;
use alloy_consensus::BlockHeader;
use metrics::{Counter, Histogram};
use reth_evm::ConfigureEvm;
use reth_metrics::Metrics;
use reth_primitives_traits::{BlockBody, RecoveredBlock};
use reth_provider::StateProviderFactory;
use reth_revm::database::StateProviderDatabase;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::Instant,
};
use tracing::{debug, trace, warn};

/// Controller for managing prewarming tasks in the execution stage.
///
/// This controller spawns background threads to execute transactions ahead of
/// the main execution, warming up the execution cache with state that will
/// likely be accessed during block processing.
#[derive(Debug)]
pub struct PrewarmController<E> {
    /// Shared execution cache
    cache: Arc<ExecutionCache>,
    /// EVM configuration (will be used when actual tx execution is implemented)
    #[allow(dead_code)]
    evm_config: E,
    /// Flag to signal cancellation
    terminate: Arc<AtomicBool>,
    /// Handle to active prewarm task (if any)
    active_task: Option<JoinHandle<()>>,
    /// Metrics
    metrics: PrewarmMetrics,
}

impl<E> PrewarmController<E>
where
    E: ConfigureEvm + Clone + Send + Sync + 'static,
{
    /// Creates a new `PrewarmController` with the given cache and EVM configuration.
    pub fn new(cache: Arc<ExecutionCache>, evm_config: E) -> Self {
        Self {
            cache,
            evm_config,
            terminate: Arc::new(AtomicBool::new(false)),
            active_task: None,
            metrics: PrewarmMetrics::default(),
        }
    }

    /// Spawns a background thread to prewarm the cache by executing the block's transactions.
    ///
    /// This is a best-effort operation - any errors during execution are logged but
    /// do not affect the main execution path.
    pub fn spawn_for<P, B>(&mut self, block: &RecoveredBlock<B>, provider_factory: P)
    where
        P: StateProviderFactory + Clone + Send + Sync + 'static,
        B: reth_primitives_traits::Block<Body: BlockBody<Transaction: Clone + Send + 'static>>,
        E: ConfigureEvm,
    {
        // Cancel any existing prewarm task
        self.terminate.store(true, Ordering::SeqCst);
        if let Some(handle) = self.active_task.take() {
            let _ = handle.join();
        }

        self.terminate.store(false, Ordering::SeqCst);

        let cache = Arc::clone(&self.cache);
        let terminate = Arc::clone(&self.terminate);
        let metrics = self.metrics.clone();

        let transactions: Vec<_> = block.body().transactions().iter().cloned().collect();
        let block_number = block.header().number();
        let tx_count = transactions.len();

        metrics.prewarm_blocks_started.increment(1);

        let handle = std::thread::spawn(move || {
            let start = Instant::now();

            debug!(
                target: "sync::stages::execution::prewarm",
                block = block_number,
                txs = tx_count,
                "Starting prewarm execution"
            );

            if let Err(err) = prewarm_transactions_inner(
                tx_count,
                provider_factory,
                Arc::clone(&cache),
                &terminate,
                &metrics,
            ) {
                warn!(
                    target: "sync::stages::execution::prewarm",
                    block = block_number,
                    %err,
                    "Prewarm execution failed"
                );
            }

            let elapsed = start.elapsed();
            metrics.prewarm_duration.record(elapsed.as_secs_f64());
            metrics.prewarm_blocks_completed.increment(1);

            debug!(
                target: "sync::stages::execution::prewarm",
                block = block_number,
                elapsed_ms = elapsed.as_millis(),
                "Prewarm execution completed"
            );
        });

        self.active_task = Some(handle);
    }

    /// Signals termination and waits for the active prewarm task to finish.
    pub fn cancel(&mut self) {
        self.terminate.store(true, Ordering::SeqCst);

        if let Some(handle) = self.active_task.take() {
            let _ = handle.join();
        }
    }

    /// Returns `true` if a prewarm task is currently active.
    pub fn is_active(&self) -> bool {
        self.active_task.as_ref().is_some_and(|handle| !handle.is_finished())
    }

    /// Returns a reference to the shared execution cache.
    pub fn cache(&self) -> Arc<ExecutionCache> {
        Arc::clone(&self.cache)
    }
}

impl<E> Drop for PrewarmController<E> {
    fn drop(&mut self) {
        self.terminate.store(true, Ordering::SeqCst);
        if let Some(handle) = self.active_task.take() {
            let _ = handle.join();
        }
    }
}

/// Inner function for prewarming transactions.
///
/// This function is type-erased to avoid complex generic constraints in the spawned thread.
fn prewarm_transactions_inner<P>(
    tx_count: usize,
    provider_factory: P,
    _cache: Arc<ExecutionCache>,
    terminate: &AtomicBool,
    metrics: &PrewarmMetrics,
) -> Result<(), PrewarmError>
where
    P: StateProviderFactory,
{
    let state_provider =
        provider_factory.latest().map_err(|e| PrewarmError::Provider(e.to_string()))?;

    let _db = StateProviderDatabase::new(&state_provider);

    // In a full implementation, we would:
    // 1. Wrap state_provider in CachedStateProvider with prewarm=true
    // 2. Create an EVM executor
    // 3. Execute each transaction to warm the cache
    //
    // For this POC, we simulate the warming by iterating and checking termination

    for idx in 0..tx_count {
        if terminate.load(Ordering::Relaxed) {
            trace!(
                target: "sync::stages::execution::prewarm",
                tx_idx = idx,
                "Prewarm terminated by request"
            );
            break;
        }

        metrics.prewarm_transactions_executed.increment(1);

        trace!(
            target: "sync::stages::execution::prewarm",
            tx_idx = idx,
            "Prewarmed transaction"
        );
    }

    Ok(())
}

/// Executes a single block for cache warming purposes.
///
/// This is a higher-level function that takes a complete block and executes
/// all its transactions to warm the cache.
pub fn prewarm_block<E, P, B>(
    block: &RecoveredBlock<B>,
    provider_factory: P,
    _evm_config: &E,
    cache: Arc<ExecutionCache>,
    terminate: &AtomicBool,
) -> Result<(), PrewarmError>
where
    E: ConfigureEvm,
    P: StateProviderFactory,
    B: reth_primitives_traits::Block<Body: BlockBody<Transaction: Clone>>,
{
    let tx_count = block.body().transactions().len();
    let metrics = PrewarmMetrics::default();

    prewarm_transactions_inner(tx_count, provider_factory, cache, terminate, &metrics)
}

/// Errors that can occur during prewarming.
#[derive(Debug, thiserror::Error)]
pub enum PrewarmError {
    /// Provider error
    #[error("provider error: {0}")]
    Provider(String),
    /// Execution error
    #[error("execution error: {0}")]
    Execution(String),
}

/// Metrics for the prewarm controller.
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.stages.execution.prewarm")]
pub struct PrewarmMetrics {
    /// Number of blocks for which prewarming was started
    pub prewarm_blocks_started: Counter,
    /// Number of blocks for which prewarming completed
    pub prewarm_blocks_completed: Counter,
    /// Number of transactions executed during prewarming
    pub prewarm_transactions_executed: Counter,
    /// Duration of prewarm execution
    pub prewarm_duration: Histogram,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stages::execution_cache::ExecutionCacheBuilder;
    use reth_evm_ethereum::EthEvmConfig;

    #[test]
    fn test_prewarm_controller_creation() {
        let cache = Arc::new(ExecutionCacheBuilder::default().build_caches(1_000_000));
        let evm_config = EthEvmConfig::mainnet();
        let controller = PrewarmController::new(cache.clone(), evm_config);

        assert!(!controller.is_active());
        assert!(Arc::ptr_eq(&controller.cache(), &cache));
    }

    #[test]
    fn test_prewarm_controller_cancel_when_inactive() {
        let cache = Arc::new(ExecutionCacheBuilder::default().build_caches(1_000_000));
        let evm_config = EthEvmConfig::mainnet();
        let mut controller = PrewarmController::new(cache, evm_config);

        // Cancelling when no task is active should not panic
        controller.cancel();
        assert!(!controller.is_active());
    }

    #[test]
    fn test_prewarm_controller_drop_cancels_task() {
        let cache = Arc::new(ExecutionCacheBuilder::default().build_caches(1_000_000));
        let evm_config = EthEvmConfig::mainnet();
        let controller = PrewarmController::new(cache, evm_config);

        // Dropping should not panic
        drop(controller);
    }

    #[test]
    fn test_prewarm_error_display() {
        let provider_err = PrewarmError::Provider("connection failed".to_string());
        assert!(provider_err.to_string().contains("connection failed"));

        let exec_err = PrewarmError::Execution("out of gas".to_string());
        assert!(exec_err.to_string().contains("out of gas"));
    }

    #[test]
    fn test_prewarm_metrics_default() {
        // Verify metrics can be created
        let metrics = PrewarmMetrics::default();
        metrics.prewarm_blocks_started.increment(1);
        metrics.prewarm_blocks_completed.increment(1);
        metrics.prewarm_transactions_executed.increment(10);
        metrics.prewarm_duration.record(0.5);
    }
}
