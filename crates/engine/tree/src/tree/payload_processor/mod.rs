//! Entrypoint for payload processing.

use crate::tree::{
    payload_processor::prewarm::{PrewarmCacheTask, PrewarmContext, PrewarmMode, PrewarmTaskEvent},
    CachedStateCacheMetrics, CachedStateMetrics, CachedStateMetricsSource, ExecutionCache,
    ExecutionEnv, PayloadExecutionCache, SavedCache, StateProviderBuilder, TreeConfig,
};
use alloy_eips::eip1898::BlockWithParent;
use alloy_primitives::B256;
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use prewarm::PrewarmMetrics;
use rayon::prelude::*;
use reth_evm::{
    ConfigureEvm, ConvertTx, ExecutableTxFor, ExecutableTxIterator, ExecutableTxParts,
    ExecutableTxTuple, TxFor, WithTxEnv,
};
use reth_execution_types::EvmState;
use reth_primitives_traits::{FastInstant as Instant, NodePrimitives};
use reth_provider::{BlockExecutionOutput, BlockReader, StateProviderFactory, StateReader};
use reth_tasks::Runtime;
pub use reth_trie_parallel::{
    error::StateRootTaskError,
    state_root_task::{
        evm_state_to_hashed_post_state, PayloadStateRootHandle, StateAccessHint,
        StateRootComputeOutcome, StateRootHandle, StateRootHintStream, StateRootMessage,
        StateRootSink, StateRootTaskCancelGuard, StateRootUpdateHook, StateRootUpdateStream,
    },
};
use std::{
    ops::Not,
    sync::{
        atomic::{AtomicBool, AtomicUsize},
        mpsc, Arc, OnceLock,
    },
};
use tracing::{debug, instrument, trace, warn, Span};

pub mod bal;
pub(crate) mod bal_prewarm_pool;
pub mod prewarm;
pub mod receipt_root_task;

/// Blocks with fewer transactions than this skip prewarming, since the fixed overhead of spawning
/// prewarm workers exceeds the execution time saved.
pub const SMALL_BLOCK_TX_THRESHOLD: usize = 5;

/// Type alias for [`PayloadHandle`] returned by payload processor spawn methods.
type IteratorTx<Evm, I> = RecoveredTx<TxFor<Evm>, <I as ExecutableTxIterator<Evm>>::Recovered>;

type IteratorPayloadHandle<Evm, I> = PayloadHandle<
    IteratorTx<Evm, I>,
    <I as ExecutableTxTuple>::Error,
    <<Evm as ConfigureEvm>::Primitives as NodePrimitives>::Receipt,
>;

type IteratorPrewarmTxReceiver<Evm, I> =
    PrewarmTxReceiver<TxFor<Evm>, <I as ExecutableTxIterator<Evm>>::Recovered>;

type IteratorExecuteTxReceiver<Evm, I> = ExecuteTxReceiver<
    TxFor<Evm>,
    <I as ExecutableTxIterator<Evm>>::Recovered,
    <I as ExecutableTxTuple>::Error,
>;

type RecoveredTx<TxEnv, Recovered> = WithTxEnv<TxEnv, Recovered>;
type IndexedTxResult<Tx, Err> = (usize, Result<Tx, Err>);
type IndexedTxReceiver<Tx, Err> = CrossbeamReceiver<IndexedTxResult<Tx, Err>>;
type IndexedTxSender<Tx, Err> = CrossbeamSender<IndexedTxResult<Tx, Err>>;
type PrewarmTxReceiver<TxEnv, Recovered> = mpsc::Receiver<(usize, RecoveredTx<TxEnv, Recovered>)>;
type ExecuteTxReceiver<TxEnv, Recovered, Err> =
    IndexedTxReceiver<RecoveredTx<TxEnv, Recovered>, Err>;
type ExecuteTxSender<TxEnv, Recovered, Err> = IndexedTxSender<RecoveredTx<TxEnv, Recovered>, Err>;

/// Entrypoint for executing the payload.
#[derive(Debug)]
pub struct PayloadProcessor<Evm>
where
    Evm: ConfigureEvm,
{
    /// The executor used by to spawn tasks.
    executor: Runtime,
    /// The most recent cache used for execution.
    execution_cache: PayloadExecutionCache,
    /// Metrics for the execution cache.
    cache_metrics: Option<CachedStateMetrics>,
    /// Metrics for shared execution cache state.
    cache_state_metrics: Option<CachedStateCacheMetrics>,
    /// Cross-block cache size in bytes.
    cross_block_cache_size: usize,
    /// Whether transactions should not be executed on prewarming task.
    disable_transaction_prewarming: bool,
    /// Whether state cache should be disable
    disable_state_cache: bool,
    /// Determines how to configure the evm for execution.
    evm_config: Evm,
    /// Whether to disable BAL-driven parallel state root computation.
    /// Only valid when BAL parallel execution is also disabled.
    disable_bal_parallel_state_root: bool,
    /// Whether BAL state prefetching during prewarm is disabled.
    disable_bal_batch_io: bool,
    /// Dedicated blocking pool for warming the BAL read-set, created lazily on the first BAL block
    /// (see [`Self::bal_prewarm_pool`]). Its threads exit when the processor is dropped.
    bal_prewarm_pool: OnceLock<Arc<bal_prewarm_pool::BalPrewarmPool>>,
}

impl<Evm> PayloadProcessor<Evm>
where
    Evm: ConfigureEvm,
{
    /// Creates a new payload processor.
    pub fn new(executor: Runtime, evm_config: Evm, config: &TreeConfig) -> Self {
        Self {
            executor,
            execution_cache: Default::default(),
            cross_block_cache_size: config.cross_block_cache_size(),
            disable_transaction_prewarming: config.disable_prewarming(),
            evm_config,
            disable_state_cache: config.disable_state_cache(),
            cache_metrics: (!config.disable_cache_metrics())
                .then(|| CachedStateMetrics::zeroed(CachedStateMetricsSource::Engine)),
            cache_state_metrics: (!config.disable_cache_metrics())
                .then(CachedStateCacheMetrics::default),
            disable_bal_parallel_state_root: config.disable_bal_parallel_state_root(),
            disable_bal_batch_io: config.disable_bal_batch_io(),
            bal_prewarm_pool: OnceLock::new(),
        }
    }

    /// Returns the dedicated BAL read-set prewarm pool, spawning its blocking worker threads on
    /// first use (only the BAL parallel execution path calls this).
    fn bal_prewarm_pool(&self) -> Arc<bal_prewarm_pool::BalPrewarmPool> {
        self.bal_prewarm_pool
            .get_or_init(|| {
                bal_prewarm_pool::BalPrewarmPool::new(bal_prewarm_pool::DEFAULT_BAL_PREWARM_THREADS)
            })
            .clone()
    }

    /// Returns the shared execution cache handle used for engine backpressure.
    pub(crate) fn execution_cache(&self) -> PayloadExecutionCache {
        self.execution_cache.clone()
    }
}

impl<Evm> PayloadProcessor<Evm>
where
    Evm: ConfigureEvm + 'static,
{
    /// Spawns transaction conversion and cache prewarming, optionally wiring prewarm output into
    /// an externally-owned state-root task.
    #[instrument(level = "debug", target = "engine::tree::payload_processor", skip_all)]
    pub fn spawn_with_state_root_streams<P, I: ExecutableTxIterator<Evm>>(
        &self,
        env: ExecutionEnv<Evm>,
        transactions: I,
        provider_builder: StateProviderBuilder<Evm::Primitives, P>,
        hint_stream: Option<StateRootHintStream>,
        hashed_update_stream: Option<StateRootUpdateStream>,
        parallel_bal_execution: bool,
    ) -> IteratorPayloadHandle<Evm, I>
    where
        P: BlockReader + StateProviderFactory + StateReader + Clone + 'static,
    {
        let (prewarm_rx, execution_rx) =
            self.spawn_tx_iterator(transactions, env.transaction_count, parallel_bal_execution);
        let prewarm_handle = self.spawn_caching_with(
            env,
            prewarm_rx,
            provider_builder,
            hint_stream,
            hashed_update_stream,
            parallel_bal_execution,
        );
        PayloadHandle { prewarm_handle, transactions: execution_rx, _span: Span::current() }
    }

    /// Transaction count threshold below which sequential conversion is used.
    ///
    /// For blocks with fewer than this many transactions, the rayon parallel iterator overhead
    /// (work-stealing setup, channel-based reorder) exceeds the cost of sequential conversion.
    /// Inspired by Nethermind's `RecoverSignature` which uses sequential `foreach` for small
    /// blocks.
    const SMALL_BLOCK_TX_THRESHOLD: usize = 30;

    /// Number of leading transactions to convert sequentially before entering the rayon
    /// parallel path.
    ///
    /// Rayon's work-stealing does not guarantee that index 0 is processed first, so the
    /// ordered consumer can block for up to ~1ms waiting for the first slot. By converting
    /// a small head sequentially and sending it immediately, execution can start without
    /// waiting for rayon scheduling.
    const PARALLEL_PREFETCH_COUNT: usize = 4;

    /// Size of the first parallel tx batch in non-BAL path.
    const FIRST_PARALLEL_TX_WINDOW_SIZE: usize = 64;

    /// Spawns a task advancing transaction env iterator and streaming updates through a channel.
    ///
    /// For blocks with fewer than [`Self::SMALL_BLOCK_TX_THRESHOLD`] transactions, uses
    /// sequential iteration to avoid rayon overhead. For larger blocks, uses rayon parallel
    /// iteration to convert transactions in parallel while streaming results to execution.
    ///
    /// When `parallel_bal_execution` is disabled, preserves the original transaction order.
    /// Otherwise, streams results as they become available.
    #[instrument(level = "debug", target = "engine::tree::payload_processor", skip_all)]
    fn spawn_tx_iterator<I: ExecutableTxIterator<Evm>>(
        &self,
        transactions: I,
        transaction_count: usize,
        parallel_bal_execution: bool,
    ) -> (IteratorPrewarmTxReceiver<Evm, I>, IteratorExecuteTxReceiver<Evm, I>) {
        let (prewarm_tx, prewarm_rx) = mpsc::sync_channel(transaction_count);
        let (execute_tx, execute_rx) = crossbeam_channel::bounded(transaction_count);

        if transaction_count == 0 {
            // Empty block — nothing to do.
        } else if transaction_count < Self::SMALL_BLOCK_TX_THRESHOLD {
            // Sequential path for small blocks — avoids rayon work-stealing setup and
            // channel-based reorder overhead when it costs more than sequential conversion.
            debug!(
                target: "engine::tree::payload_processor",
                transaction_count,
                "using sequential sig recovery for small block"
            );
            self.executor.spawn_blocking_named("tx-iterator", move || {
                let (transactions, convert) = transactions.into_parts();
                convert_serial(transactions.into_iter(), &convert, &prewarm_tx, &execute_tx);
            });
        } else {
            // Parallel path — recover signatures in parallel on rayon, stream results
            // to prewarming and execution.
            let executor = self.executor.clone();
            self.executor.spawn_blocking_named("tx-iterator", move || {
                let (transactions, convert) = transactions.into_parts();
                if parallel_bal_execution {
                    // With BALs, we don't care about the order of transactions in execution and
                    // prewarming, so we don't have to use `for_each_ordered_in`.
                    executor.cpu_pool().install(|| {
                        transactions
                            .into_par_iter()
                            .enumerate()
                            .map(|(i, tx)| {
                                let tx = convert.convert(tx);
                                (i, tx)
                            })
                            .for_each(|(idx, tx)| {
                                let tx = tx.map(|tx| {
                                    let tx = WithTxEnv::new(tx);
                                    let _ = prewarm_tx.send((idx, tx.clone()));
                                    tx
                                });
                                let _ = execute_tx.send((idx, tx));
                                trace!(target: "engine::tree::payload_processor", idx, "yielded transaction");
                            });
                    });
                } else {
                    // To avoid a ~1ms stall waiting for rayon to schedule index 0, the first
                    // few transactions are recovered sequentially and sent immediately before
                    // entering the parallel iterator for the remainder.
                    let prefetch = Self::PARALLEL_PREFETCH_COUNT.min(transaction_count);
                    let mut iter = transactions.into_iter();

                    // Convert the first few transactions sequentially so execution can
                    // start immediately without waiting for rayon work-stealing.
                    convert_serial(iter.by_ref().take(prefetch), &convert, &prewarm_tx, &execute_tx);

                    let mut iter = iter.enumerate();

                    let mut batch_size = Self::FIRST_PARALLEL_TX_WINDOW_SIZE;

                    // Without BALs, we need to preserve the initial order of transactions.
                    // Process exponentially increasing windows to make sure that first transactions are prioritized.
                    executor.cpu_pool().install(move || {
                        loop {
                            let chunk = iter
                                .by_ref()
                                .take(batch_size)
                                .collect::<Vec<_>>();
                            if chunk.is_empty() {
                                break;
                            }

                            batch_size = batch_size.saturating_mul(2);

                            let chunk = chunk
                                .into_par_iter()
                                .map(|(i, tx)| {
                                    let idx = i + prefetch;
                                    let tx = convert.convert(tx).map(WithTxEnv::new);
                                    (idx, tx)
                                })
                                .collect::<Vec<_>>();

                            for (idx, tx) in chunk {
                                if let Ok(tx) = &tx {
                                    let _ = prewarm_tx.send((idx, tx.clone()));
                                }
                                let _ = execute_tx.send((idx, tx));
                                trace!(target: "engine::tree::payload_processor", idx, "yielded transaction");
                            }
                        }
                    });
                }
            });
        }

        (prewarm_rx, execute_rx)
    }

    /// Spawn prewarming optionally wired to the sparse trie task for target updates.
    ///
    /// `parallel_bal_execution` is true when the BAL execute path will execute this block. In
    /// that case prewarm runs in BAL mode: it streams BAL-derived sparse-trie updates and,
    /// unless `disable_bal_batch_io` is set, prefetches BAL-declared state into the shared cache.
    #[instrument(level = "debug", target = "engine::tree::payload_processor", skip_all)]
    fn spawn_caching_with<P>(
        &self,
        env: ExecutionEnv<Evm>,
        transactions: mpsc::Receiver<(usize, impl ExecutableTxFor<Evm> + Clone + Send + 'static)>,
        provider_builder: StateProviderBuilder<Evm::Primitives, P>,
        hint_stream: Option<StateRootHintStream>,
        hashed_update_stream: Option<StateRootUpdateStream>,
        parallel_bal_execution: bool,
    ) -> CacheTaskHandle<<Evm::Primitives as NodePrimitives>::Receipt>
    where
        P: BlockReader + StateProviderFactory + StateReader + Clone + 'static,
    {
        // Each mode carries the capability its producers use; the rest is dropped here, so
        // unused capabilities do not keep the state-root task's update channel open.
        let mode = if parallel_bal_execution {
            PrewarmMode::BlockAccessList {
                bal: env.decoded_bal.clone().expect("BAL dispatch implies decoded BAL"),
                updates: hashed_update_stream,
            }
        } else if self.disable_transaction_prewarming ||
            env.transaction_count < SMALL_BLOCK_TX_THRESHOLD
        {
            PrewarmMode::Skipped
        } else {
            PrewarmMode::Transactions { pending: transactions, hints: hint_stream }
        };
        let saved_cache = self.disable_state_cache.not().then(|| self.cache_for(env.parent_hash));

        let executed_tx_index = Arc::new(AtomicUsize::new(0));
        // configure prewarming
        let prewarm_ctx = PrewarmContext {
            env,
            evm_config: self.evm_config.clone(),
            saved_cache: saved_cache.clone(),
            provider: provider_builder,
            bal_prewarm_pool: parallel_bal_execution.then(|| self.bal_prewarm_pool()),
            metrics: PrewarmMetrics::default(),
            cache_metrics: self.cache_metrics.clone(),
            cache_state_metrics: self.cache_state_metrics.clone(),
            terminate_execution: Arc::new(AtomicBool::new(false)),
            executed_tx_index: Arc::clone(&executed_tx_index),
            disable_bal_parallel_state_root: self.disable_bal_parallel_state_root,
            disable_bal_batch_io: self.disable_bal_batch_io,
        };

        let (prewarm_task, to_prewarm_task) =
            PrewarmCacheTask::new(self.executor.clone(), self.execution_cache.clone(), prewarm_ctx);
        {
            let to_prewarm_task = to_prewarm_task.clone();
            self.executor.spawn_blocking_named("prewarm", move || {
                prewarm_task.run(mode, to_prewarm_task);
            });
        }

        CacheTaskHandle {
            saved_cache,
            to_prewarm_task: Some(to_prewarm_task),
            executed_tx_index,
            cache_metrics: self.cache_metrics.clone(),
        }
    }

    /// Returns the cache for the given parent hash.
    ///
    /// If the given hash is different then what is recently cached, then this will create a new
    /// instance.
    #[instrument(level = "debug", target = "engine::caching", skip(self))]
    pub fn cache_for(&self, parent_hash: B256) -> SavedCache {
        if let Some(cache) = self.execution_cache.get_cache_for(parent_hash) {
            debug!("reusing execution cache");
            cache
        } else {
            debug!("creating new execution cache on cache miss");
            let start = Instant::now();
            let cache = ExecutionCache::new(self.cross_block_cache_size);
            if let Some(metrics) = &self.cache_metrics {
                metrics.record_cache_creation(start.elapsed());
            }
            SavedCache::new(parent_hash, cache)
        }
    }

    /// Updates the execution cache with the post-execution state from an inserted block.
    ///
    /// This is used when blocks are inserted directly (e.g., locally built blocks by sequencers)
    /// to ensure the cache remains warm for subsequent block execution.
    ///
    /// The cache enables subsequent blocks to reuse account, storage, and bytecode data without
    /// hitting the database, maintaining performance consistency.
    pub fn on_inserted_executed_block(
        &self,
        block_with_parent: BlockWithParent,
        block_state: &EvmState,
    ) {
        let cache_state_metrics = self.cache_state_metrics.clone();
        self.execution_cache.update_with_guard(|cached| {
            if cached.as_ref().is_some_and(|c| c.executed_block_hash() != block_with_parent.parent) {
                debug!(
                    target: "engine::caching",
                    parent_hash = %block_with_parent.parent,
                    "Cannot find cache for parent hash, skip updating cache with new state for inserted executed block",
                );
                return
            }

            if let Some(cache) = cached.as_ref().filter(|cache| !cache.is_available()) {
                debug!(
                    target: "engine::caching",
                    parent_hash = %block_with_parent.parent,
                    usage_count = cache.usage_count(),
                    "Execution cache is in use, skip updating cache with new state for inserted executed block",
                );
                return
            }

            // Take existing cache (if any) or create fresh caches
            let caches = match cached.take() {
                Some(existing) => existing.cache().clone(),
                None => ExecutionCache::new(self.cross_block_cache_size),
            };

            // Insert the block's state into cache
            let new_cache = SavedCache::new(block_with_parent.block.hash, caches);
            if new_cache.cache().insert_state(block_state).is_err() {
                *cached = None;
                debug!(target: "engine::caching", "cleared execution cache on update error");
                return
            }
            new_cache.update_metrics(cache_state_metrics.as_ref());

            // Replace with the updated cache
            *cached = Some(new_cache);
            debug!(target: "engine::caching", ?block_with_parent, "Updated execution cache for inserted block");
        });
    }
}

/// Converts transactions sequentially and sends them to the prewarm and execute channels.
fn convert_serial<RawTx, Tx, TxEnv, InnerTx, Recovered, Err, C>(
    iter: impl Iterator<Item = RawTx>,
    convert: &C,
    prewarm_tx: &mpsc::SyncSender<(usize, WithTxEnv<TxEnv, Recovered>)>,
    execute_tx: &ExecuteTxSender<TxEnv, Recovered, Err>,
) where
    Tx: ExecutableTxParts<TxEnv, InnerTx, Recovered = Recovered>,
    TxEnv: Clone,
    C: ConvertTx<RawTx, Tx = Tx, Error = Err>,
{
    for (idx, raw_tx) in iter.enumerate() {
        let tx = convert.convert(raw_tx);
        let tx = tx.map(|tx| WithTxEnv::new(tx));
        if let Ok(tx) = &tx {
            let _ = prewarm_tx.send((idx, tx.clone()));
        }
        let _ = execute_tx.send((idx, tx));
        trace!(target: "engine::tree::payload_processor", idx, "yielded transaction");
    }
}

/// Handle to all the spawned tasks.
///
/// Generic over `R` (receipt type) to allow sharing `Arc<ExecutionOutcome<R>>` with the
/// caching task without cloning the execution state.
#[derive(Debug)]
pub struct PayloadHandle<Tx, Err, R> {
    prewarm_handle: CacheTaskHandle<R>,
    /// Stream of block transactions and their indices in the block.
    transactions: IndexedTxReceiver<Tx, Err>,
    /// Span for tracing
    _span: Span,
}

impl<Tx, Err, R: Send + Sync + 'static> PayloadHandle<Tx, Err, R> {
    /// Returns a clone of the caches used by prewarming
    pub fn caches(&self) -> Option<ExecutionCache> {
        self.prewarm_handle.saved_cache.as_ref().map(|cache| cache.cache().clone())
    }

    /// Returns engine cache metrics if a cache exists for prewarming.
    pub fn cache_metrics(&self) -> Option<CachedStateMetrics> {
        self.prewarm_handle.cache_metrics.clone()
    }

    /// Returns a reference to the shared executed transaction index counter.
    ///
    /// The main execution loop should store `index + 1` after executing each transaction so that
    /// prewarm workers can skip transactions that have already been processed.
    pub const fn executed_tx_index(&self) -> &Arc<AtomicUsize> {
        &self.prewarm_handle.executed_tx_index
    }

    /// Terminates the pre-warming transaction processing.
    ///
    /// Note: This does not terminate the task yet.
    pub fn stop_prewarming_execution(&self) {
        self.prewarm_handle.stop_prewarming_execution()
    }

    /// Terminates the entire caching task.
    ///
    /// If the [`BlockExecutionOutput`] is provided it will update the shared cache using its
    /// bundle state. Using `Arc<ExecutionOutcome>` allows sharing with the main execution
    /// path without cloning the execution state.
    ///
    /// Returns a sender for the channel that should be notified on block validation success.
    pub fn terminate_caching(
        &mut self,
        execution_outcome: Option<Arc<BlockExecutionOutput<R>>>,
    ) -> Option<mpsc::Sender<()>> {
        self.prewarm_handle.terminate_caching(execution_outcome)
    }

    /// Returns iterator yielding transactions from the stream.
    pub fn iter_transactions(&mut self) -> impl Iterator<Item = Result<Tx, Err>> + '_ {
        self.transactions.iter().map(|(_, tx)| tx)
    }

    /// Returns a clone of the indexed transaction receiver.
    pub fn clone_transaction_receiver(&self) -> IndexedTxReceiver<Tx, Err> {
        self.transactions.clone()
    }
}

/// Access to the spawned [`PrewarmCacheTask`].
///
/// Generic over `R` (receipt type) to allow sharing `Arc<ExecutionOutcome<R>>` with the
/// prewarm task without cloning the execution state.
#[derive(Debug)]
pub struct CacheTaskHandle<R> {
    /// The shared cache the task operates with.
    saved_cache: Option<SavedCache>,
    /// Channel to the spawned prewarm task if any
    to_prewarm_task: Option<std::sync::mpsc::Sender<PrewarmTaskEvent<R>>>,
    /// Shared counter tracking the next transaction index to be executed by the main execution
    /// loop. Prewarm workers skip transactions below this index.
    executed_tx_index: Arc<AtomicUsize>,
    /// Metrics for the execution cache.
    cache_metrics: Option<CachedStateMetrics>,
}

impl<R: Send + Sync + 'static> CacheTaskHandle<R> {
    /// Terminates the pre-warming transaction processing.
    ///
    /// Note: This does not terminate the task yet.
    pub fn stop_prewarming_execution(&self) {
        self.to_prewarm_task
            .as_ref()
            .map(|tx| tx.send(PrewarmTaskEvent::TerminateTransactionExecution).ok());
    }

    /// Terminates the entire pre-warming task.
    ///
    /// If the [`BlockExecutionOutput`] is provided it will update the shared cache using its
    /// execution state. Using `Arc<ExecutionOutcome>` avoids cloning it.
    #[must_use = "sender must be used and notified on block validation success"]
    pub fn terminate_caching(
        &mut self,
        execution_outcome: Option<Arc<BlockExecutionOutput<R>>>,
    ) -> Option<mpsc::Sender<()>> {
        if let Some(tx) = self.to_prewarm_task.take() {
            let (valid_block_tx, valid_block_rx) = mpsc::channel();
            let event = PrewarmTaskEvent::Terminate { execution_outcome, valid_block_rx };
            let _ = tx.send(event);

            Some(valid_block_tx)
        } else {
            None
        }
    }
}

impl<R> Drop for CacheTaskHandle<R> {
    fn drop(&mut self) {
        // Ensure we always terminate on drop - send None without needing Send + Sync bounds
        if let Some(tx) = self.to_prewarm_task.take() {
            let _ = tx.send(PrewarmTaskEvent::Terminate {
                execution_outcome: None,
                valid_block_rx: mpsc::channel().1,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::tree::{
        payload_processor::PayloadProcessor, ExecutionCache, PayloadExecutionCache, SavedCache,
        TreeConfig,
    };
    use alloy_consensus::constants::KECCAK_EMPTY;
    use alloy_eips::eip1898::{BlockNumHash, BlockWithParent};
    use alloy_primitives::{Address, B256, U256};
    use reth_chainspec::ChainSpec;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_execution_cache::CachedStatus;
    use reth_execution_types::{execution_state_from_init, EvmState};
    use reth_primitives_traits::Account;
    use std::sync::Arc;

    fn make_saved_cache(hash: B256) -> SavedCache {
        let execution_cache = ExecutionCache::new(1_000);
        SavedCache::new(hash, execution_cache)
    }

    #[test]
    fn execution_cache_allows_single_checkout() {
        let execution_cache = PayloadExecutionCache::default();
        let hash = B256::from([1u8; 32]);

        execution_cache.update_with_guard(|slot| *slot = Some(make_saved_cache(hash)));

        let first = execution_cache.get_cache_for(hash);
        assert!(first.is_some(), "expected initial checkout to succeed");

        let second = execution_cache.get_cache_for(hash);
        assert!(second.is_none(), "second checkout should be blocked while guard is active");

        drop(first);

        let third = execution_cache.get_cache_for(hash);
        assert!(third.is_some(), "third checkout should succeed after guard is dropped");
    }

    #[test]
    fn execution_cache_checkout_releases_on_drop() {
        let execution_cache = PayloadExecutionCache::default();
        let hash = B256::from([2u8; 32]);

        execution_cache.update_with_guard(|slot| *slot = Some(make_saved_cache(hash)));

        {
            let guard = execution_cache.get_cache_for(hash);
            assert!(guard.is_some(), "expected checkout to succeed");
            // Guard dropped at end of scope
        }

        let retry = execution_cache.get_cache_for(hash);
        assert!(retry.is_some(), "checkout should succeed after guard drop");
    }

    #[test]
    fn execution_cache_mismatch_parent_clears_and_returns() {
        let execution_cache = PayloadExecutionCache::default();
        let hash = B256::from([3u8; 32]);

        execution_cache.update_with_guard(|slot| *slot = Some(make_saved_cache(hash)));

        // When the parent hash doesn't match (fork block), the cache is cleared,
        // hash updated on the original, and clone returned for reuse
        let different_hash = B256::from([4u8; 32]);
        let cache = execution_cache.get_cache_for(different_hash);
        assert!(cache.is_some(), "cache should be returned for reuse after clearing");

        drop(cache);

        // The stored cache now has the fork block's parent hash.
        // Canonical chain looking for original hash sees a mismatch → clears and reuses.
        let original = execution_cache.get_cache_for(hash);
        assert!(original.is_some(), "canonical chain gets cache back via mismatch+clear");
    }

    #[test]
    fn execution_cache_update_after_release_succeeds() {
        let execution_cache = PayloadExecutionCache::default();
        let initial = B256::from([5u8; 32]);

        execution_cache.update_with_guard(|slot| *slot = Some(make_saved_cache(initial)));

        let guard =
            execution_cache.get_cache_for(initial).expect("expected initial checkout to succeed");

        drop(guard);

        let updated = B256::from([6u8; 32]);
        execution_cache.update_with_guard(|slot| *slot = Some(make_saved_cache(updated)));

        let new_checkout = execution_cache.get_cache_for(updated);
        assert!(new_checkout.is_some(), "new checkout should succeed after release and update");
    }

    #[test]
    fn on_inserted_executed_block_populates_cache() {
        let payload_processor = PayloadProcessor::new(
            reth_tasks::Runtime::test(),
            EthEvmConfig::new(Arc::new(ChainSpec::default())),
            &TreeConfig::default(),
        );

        let parent_hash = B256::from([1u8; 32]);
        let block_hash = B256::from([10u8; 32]);
        let block_with_parent = BlockWithParent {
            block: BlockNumHash { hash: block_hash, number: 1 },
            parent: parent_hash,
        };
        let block_state = EvmState::default();

        // Cache should be empty initially
        assert!(payload_processor.execution_cache.get_cache_for(block_hash).is_none());

        // Update cache with inserted block
        payload_processor.on_inserted_executed_block(block_with_parent, &block_state);

        // Cache should now exist for the block hash
        let cached = payload_processor.execution_cache.get_cache_for(block_hash);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().executed_block_hash(), block_hash);
    }

    #[test]
    fn on_inserted_executed_block_skips_on_parent_mismatch() {
        let payload_processor = PayloadProcessor::new(
            reth_tasks::Runtime::test(),
            EthEvmConfig::new(Arc::new(ChainSpec::default())),
            &TreeConfig::default(),
        );

        // Setup: populate cache with block 1
        let block1_hash = B256::from([1u8; 32]);
        payload_processor
            .execution_cache
            .update_with_guard(|slot| *slot = Some(make_saved_cache(block1_hash)));

        // Try to insert block 3 with wrong parent (should skip and keep block 1's cache)
        let wrong_parent = B256::from([99u8; 32]);
        let block3_hash = B256::from([3u8; 32]);
        let block_with_parent = BlockWithParent {
            block: BlockNumHash { hash: block3_hash, number: 3 },
            parent: wrong_parent,
        };
        let block_state = EvmState::default();

        payload_processor.on_inserted_executed_block(block_with_parent, &block_state);

        // Cache should still be for block 1 (unchanged)
        let cached = payload_processor.execution_cache.get_cache_for(block1_hash);
        assert!(cached.is_some(), "Original cache should be preserved");

        // Cache for block 3 should not exist
        let cached3 = payload_processor.execution_cache.get_cache_for(block3_hash);
        assert!(cached3.is_none(), "New block cache should not be created on mismatch");
    }

    #[test]
    fn on_inserted_executed_block_does_not_mutate_checked_out_parent_cache() {
        let payload_processor = PayloadProcessor::new(
            reth_tasks::Runtime::test(),
            EthEvmConfig::new(Arc::new(ChainSpec::default())),
            &TreeConfig::default(),
        );

        let parent_hash = B256::from([1u8; 32]);
        payload_processor
            .execution_cache
            .update_with_guard(|slot| *slot = Some(make_saved_cache(parent_hash)));

        // Checking out the cache bumps its `ExecutionCache` refcount, marking the slot as in-use.
        // The returned SavedCache shares the same underlying ExecutionCache Arc as the slot,
        // so any writes through the slot are observable here
        let checked_out = payload_processor
            .execution_cache
            .get_cache_for(parent_hash)
            .expect("expected parent cache checkout to succeed");

        let polluted_address = Address::random();
        let block_state = execution_state_from_init(
            [(
                polluted_address,
                (
                    None,
                    Some(Account {
                        balance: U256::from(1337),
                        nonce: 7,
                        bytecode_hash: Some(KECCAK_EMPTY),
                    }),
                    Default::default(),
                ),
            )],
            [],
        );

        // Make parent match the cached slot so we bypass the parent-mismatch guard and exercise
        // the in-use guard specifically.
        let block_with_parent = BlockWithParent {
            block: BlockNumHash { hash: B256::from([2u8; 32]), number: 2 },
            parent: parent_hash,
        };

        payload_processor.on_inserted_executed_block(block_with_parent, &block_state);

        // The closure runs only on a cache miss, so NotCached(None) means polluted_address was
        // absent and Cached(Some(_)) means it was written by on_inserted_executed_block.
        let account = checked_out
            .cache()
            .get_or_try_insert_account_with(polluted_address, || Ok::<_, ()>(None))
            .expect("cache read should succeed");

        assert_eq!(
            account,
            CachedStatus::NotCached(None),
            "checked-out parent cache should not observe state from inserted local block"
        );
    }

    /// Tests the full prewarm lifecycle for a fork block:
    ///
    /// 1. Cache is at canonical block 4.
    /// 2. Fork block (parent = block 2) checks out the cache via `get_cache_for`, simulating what
    ///    `PrewarmCacheTask` does when it receives a `SavedCache`.
    /// 3. Prewarm populates the shared cache with fork-specific state.
    /// 4. While the prewarm clone is alive, the cache is unavailable (`usage_count` > 1).
    /// 5. Prewarm drops without calling `save_cache` (fork block was invalid).
    /// 6. Canonical block 5 (parent = block 4) must get a cache with correct hash and no stale fork
    ///    data.
    #[test]
    fn fork_prewarm_dropped_without_save_does_not_corrupt_cache() {
        let execution_cache = PayloadExecutionCache::default();

        // Canonical chain at block 4.
        let block4_hash = B256::from([4u8; 32]);
        execution_cache.update_with_guard(|slot| *slot = Some(make_saved_cache(block4_hash)));

        // Fork block arrives with parent = block 2. Prewarm task checks out the cache.
        // This simulates PrewarmCacheTask receiving a SavedCache clone from get_cache_for.
        let fork_parent = B256::from([2u8; 32]);
        let prewarm_cache = execution_cache.get_cache_for(fork_parent);
        assert!(prewarm_cache.is_some(), "prewarm should obtain cache for fork block");
        let prewarm_cache = prewarm_cache.unwrap();
        assert_eq!(prewarm_cache.executed_block_hash(), fork_parent);

        // Prewarm populates cache with fork-specific state (ancestor data for block 2).
        // Since ExecutionCache uses Arc<Inner>, this data is shared with the stored original.
        let fork_addr = Address::from([0xBB; 20]);
        let fork_key = B256::from([0xCC; 32]);
        prewarm_cache.cache().insert_storage(fork_addr, fork_key, Some(U256::from(999)));

        // While prewarm holds the clone, the cache handle count > 1 so the cache is in use.
        let during_prewarm = execution_cache.get_cache_for(block4_hash);
        assert!(
            during_prewarm.is_none(),
            "cache must be unavailable while prewarm holds a reference"
        );

        // Fork block fails — prewarm task drops without calling save_cache/update_with_guard.
        drop(prewarm_cache);

        // Canonical block 5 arrives (parent = block 4).
        // Stored hash = fork_parent (our fix), so get_cache_for sees a mismatch,
        // clears the stale fork data, and returns a cache with hash = block4_hash.
        let block5_cache = execution_cache.get_cache_for(block4_hash);
        assert!(
            block5_cache.is_some(),
            "canonical chain must get cache after fork prewarm is dropped"
        );
        assert_eq!(
            block5_cache.as_ref().unwrap().executed_block_hash(),
            block4_hash,
            "cache must carry the canonical parent hash, not the fork parent"
        );
    }
}
