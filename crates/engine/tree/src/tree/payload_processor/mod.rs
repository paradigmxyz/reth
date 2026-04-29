//! Entrypoint for payload processing.

use super::precompile_cache::PrecompileCacheMap;
use crate::tree::{
    payload_processor::prewarm::{PrewarmCacheTask, PrewarmContext, PrewarmMode, PrewarmTaskEvent},
    sparse_trie::SparseTrieCacheTask,
    CacheWaitDurations, CachedStateMetrics, CachedStateMetricsSource, ExecutionCache,
    PayloadExecutionCache, SavedCache, StateProviderBuilder, TreeConfig, WaitForCaches,
};
use alloy_eip7928::bal::DecodedBal;
use alloy_eips::{eip1898::BlockWithParent, eip4895::Withdrawal};
use alloy_primitives::B256;
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use multiproof::*;
use prewarm::PrewarmMetrics;
use rayon::prelude::*;
use reth_evm::{
    block::ExecutableTxParts,
    execute::{ExecutableTxFor, WithTxEnv},
    ConfigureEvm, ConvertTx, EvmEnvFor, ExecutableTxIterator, ExecutableTxTuple, OnStateHook,
    SpecFor, TxEnvFor,
};
use reth_primitives_traits::{FastInstant as Instant, NodePrimitives};
use reth_provider::{
    BlockExecutionOutput, BlockReader, DatabaseProviderROFactory, StateProviderFactory, StateReader,
};
use reth_revm::db::BundleState;
use reth_tasks::{utils::increase_thread_priority, ForEachOrdered, Runtime};
use reth_trie::{hashed_cursor::HashedCursorFactory, trie_cursor::TrieCursorFactory};
use reth_trie_parallel::{
    proof_task::{ProofTaskCtx, ProofWorkerHandle},
    root::ParallelStateRootError,
};
use reth_trie_sparse::{
    ArenaParallelSparseTrie, ConfigurableSparseTrie, RevealableSparseTrie, SparseStateTrie,
};
use std::{
    ops::Not,
    sync::{
        atomic::{AtomicBool, AtomicUsize},
        mpsc::{self, channel},
        Arc,
    },
};
use tracing::{debug, debug_span, instrument, trace, warn, Span};

pub mod multiproof;
mod preserved_sparse_trie;
pub mod prewarm;
pub mod receipt_root_task;
pub mod sparse_trie;

use preserved_sparse_trie::{PreservedSparseTrie, SharedPreservedSparseTrie};

/// Default node capacity for shrinking the sparse trie. This is used to limit the number of trie
/// nodes in allocated sparse tries.
///
/// Node maps have a key of `Nibbles` and value of `SparseNode`.
/// The `size_of::<Nibbles>` is 40, and `size_of::<SparseNode>` is 80.
///
/// If we have 1 million entries of 120 bytes each, this conservative estimate comes out at around
/// 120MB.
pub const SPARSE_TRIE_MAX_NODES_SHRINK_CAPACITY: usize = 1_000_000;

/// Default value capacity for shrinking the sparse trie. This is used to limit the number of values
/// in allocated sparse tries.
///
/// There are storage and account values, the largest of the two being account values, which are
/// essentially `TrieAccount`s.
///
/// Account value maps have a key of `Nibbles` and value of `TrieAccount`.
/// The `size_of::<Nibbles>` is 40, and `size_of::<TrieAccount>` is 104.
///
/// If we have 1 million entries of 144 bytes each, this conservative estimate comes out at around
/// 144MB.
pub const SPARSE_TRIE_MAX_VALUES_SHRINK_CAPACITY: usize = 1_000_000;

/// Blocks with fewer transactions than this skip prewarming, since the fixed overhead of spawning
/// prewarm workers exceeds the execution time saved.
pub const SMALL_BLOCK_TX_THRESHOLD: usize = 5;

/// Type alias for [`PayloadHandle`] returned by payload processor spawn methods.
type IteratorPayloadHandle<Evm, I, N> = PayloadHandle<
    WithTxEnv<TxEnvFor<Evm>, <I as ExecutableTxIterator<Evm>>::Recovered>,
    <I as ExecutableTxTuple>::Error,
    <N as NodePrimitives>::Receipt,
>;

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
    /// Metrics for trie operations
    trie_metrics: MultiProofTaskMetrics,
    /// Cross-block cache size in bytes.
    cross_block_cache_size: usize,
    /// Whether transactions should not be executed on prewarming task.
    disable_transaction_prewarming: bool,
    /// Whether state cache should be disable
    disable_state_cache: bool,
    /// Determines how to configure the evm for execution.
    evm_config: Evm,
    /// Whether precompile cache should be disabled.
    precompile_cache_disabled: bool,
    /// Precompile cache map.
    precompile_cache_map: PrecompileCacheMap<SpecFor<Evm>>,
    /// A pruned `SparseStateTrie`, kept around as a cache of already revealed trie nodes and to
    /// re-use allocated memory. Stored with the block hash it was computed for to enable trie
    /// preservation across sequential payload validations.
    sparse_state_trie: SharedPreservedSparseTrie,
    /// LFU hot-slot capacity: max storage slots retained across prune cycles.
    sparse_trie_max_hot_slots: usize,
    /// LFU hot-account capacity: max account addresses retained across prune cycles.
    sparse_trie_max_hot_accounts: usize,
    /// Whether sparse trie cache pruning is fully disabled.
    disable_sparse_trie_cache_pruning: bool,
    /// Whether to disable BAL-based parallel execution (falls back to tx-based prewarming).
    disable_bal_parallel_execution: bool,
    /// Whether to disable BAL-driven parallel state root computation.
    disable_bal_parallel_state_root: bool,
    /// Whether BAL batched IO is disabled.
    disable_bal_batch_io: bool,
}

impl<N, Evm> PayloadProcessor<Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    /// Returns a reference to the workload executor driving payload tasks.
    pub const fn executor(&self) -> &Runtime {
        &self.executor
    }

    /// Creates a new payload processor.
    pub fn new(
        executor: Runtime,
        evm_config: Evm,
        config: &TreeConfig,
        precompile_cache_map: PrecompileCacheMap<SpecFor<Evm>>,
    ) -> Self {
        Self {
            executor,
            execution_cache: Default::default(),
            trie_metrics: Default::default(),
            cross_block_cache_size: config.cross_block_cache_size(),
            disable_transaction_prewarming: config.disable_prewarming(),
            evm_config,
            disable_state_cache: config.disable_state_cache(),
            precompile_cache_disabled: config.precompile_cache_disabled(),
            precompile_cache_map,
            sparse_state_trie: SharedPreservedSparseTrie::default(),
            sparse_trie_max_hot_slots: config.sparse_trie_max_hot_slots(),
            sparse_trie_max_hot_accounts: config.sparse_trie_max_hot_accounts(),
            disable_sparse_trie_cache_pruning: config.disable_sparse_trie_cache_pruning(),
            cache_metrics: (!config.disable_cache_metrics())
                .then(|| CachedStateMetrics::zeroed(CachedStateMetricsSource::Engine)),
            disable_bal_parallel_execution: config.disable_bal_parallel_execution(),
            disable_bal_parallel_state_root: config.disable_bal_parallel_state_root(),
            disable_bal_batch_io: config.disable_bal_batch_io(),
        }
    }
}

impl<Evm> WaitForCaches for PayloadProcessor<Evm>
where
    Evm: ConfigureEvm,
{
    fn wait_for_caches(&self) -> CacheWaitDurations {
        debug!(target: "engine::tree::payload_processor", "Waiting for execution cache and sparse trie locks");

        // Wait for both caches in parallel using std threads
        let execution_cache = self.execution_cache.clone();
        let sparse_trie = self.sparse_state_trie.clone();

        // Use channels and spawn_blocking instead of std::thread::spawn
        let (execution_tx, execution_rx) = std::sync::mpsc::channel();
        let (sparse_trie_tx, sparse_trie_rx) = std::sync::mpsc::channel();

        self.executor.spawn_blocking_named("wait-exec-cache", move || {
            let _ = execution_tx.send(execution_cache.wait_for_availability());
        });
        self.executor.spawn_blocking_named("wait-sparse-tri", move || {
            let _ = sparse_trie_tx.send(sparse_trie.wait_for_availability());
        });

        let execution_cache_duration =
            execution_rx.recv().expect("execution cache wait task failed to send result");
        let sparse_trie_duration =
            sparse_trie_rx.recv().expect("sparse trie wait task failed to send result");

        debug!(
            target: "engine::tree::payload_processor",
            ?execution_cache_duration,
            ?sparse_trie_duration,
            "Execution cache and sparse trie locks acquired"
        );
        CacheWaitDurations {
            execution_cache: execution_cache_duration,
            sparse_trie: sparse_trie_duration,
        }
    }
}

impl<N, Evm> PayloadProcessor<Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    /// Spawns all background tasks and returns a handle connected to the tasks.
    ///
    /// - Transaction prewarming task
    /// - State root task
    /// - Sparse trie task
    ///
    /// # Transaction prewarming task
    ///
    /// Responsible for feeding state updates to the sparse trie task.
    ///
    /// This task runs until:
    ///  - externally cancelled (e.g. sequential block execution is complete)
    ///
    /// ## Sparse trie task
    ///
    /// Responsible for calculating the state root.
    ///
    /// This task runs until there are no further updates to process.
    ///
    ///
    /// This returns a handle to await the final state root and to interact with the tasks (e.g.
    /// canceling)
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor",
        name = "payload processor",
        skip_all
    )]
    pub fn spawn<P, F, I: ExecutableTxIterator<Evm>>(
        &mut self,
        env: ExecutionEnv<Evm>,
        transactions: I,
        provider_builder: StateProviderBuilder<N, P>,
        multiproof_provider_factory: F,
        config: &TreeConfig,
    ) -> IteratorPayloadHandle<Evm, I, N>
    where
        P: BlockReader + StateProviderFactory + StateReader + Clone + 'static,
        F: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
            + Clone
            + Send
            + Sync
            + 'static,
    {
        // start preparing transactions immediately
        let (prewarm_rx, execution_rx) =
            self.spawn_tx_iterator(transactions, env.transaction_count);

        let span = Span::current();

        let halve_workers = env.transaction_count <= Self::SMALL_BLOCK_PROOF_WORKER_TX_THRESHOLD;
        let state_root_handle = self.spawn_state_root(
            multiproof_provider_factory,
            env.parent_state_root,
            halve_workers,
            config,
        );
        let install_state_hook = env.decoded_bal.is_none();
        let prewarm_handle = self.spawn_caching_with(
            env,
            prewarm_rx,
            provider_builder,
            Some(state_root_handle.updates_tx().clone()),
        );

        PayloadHandle {
            state_root_handle: Some(state_root_handle),
            install_state_hook,
            prewarm_handle,
            transactions: execution_rx,
            _span: span,
        }
    }

    /// Spawns a task that exclusively handles cache prewarming for transaction execution.
    ///
    /// Returns a [`PayloadHandle`] to communicate with the task.
    #[instrument(level = "debug", target = "engine::tree::payload_processor", skip_all)]
    pub fn spawn_cache_exclusive<P, I: ExecutableTxIterator<Evm>>(
        &self,
        env: ExecutionEnv<Evm>,
        transactions: I,
        provider_builder: StateProviderBuilder<N, P>,
    ) -> IteratorPayloadHandle<Evm, I, N>
    where
        P: BlockReader + StateProviderFactory + StateReader + Clone + 'static,
    {
        let (prewarm_rx, execution_rx) =
            self.spawn_tx_iterator(transactions, env.transaction_count);
        let prewarm_handle = self.spawn_caching_with(env, prewarm_rx, provider_builder, None);
        PayloadHandle {
            state_root_handle: None,
            install_state_hook: false,
            prewarm_handle,
            transactions: execution_rx,
            _span: Span::current(),
        }
    }

    /// Spawns state root computation pipeline (multiproof + sparse trie tasks).
    ///
    /// The returned [`StateRootHandle`] provides:
    /// - [`StateRootHandle::state_hook`] — an [`OnStateHook`] to stream state updates during
    ///   execution.
    /// - [`StateRootHandle::state_root`] — blocks until the state root is computed and returns the
    ///   state root.
    ///
    /// The state hook **must** be dropped after execution to signal the end of state updates.
    ///
    /// When `halve_workers` is true, the proof worker pool is halved (for small blocks where
    /// fewer transactions produce fewer state changes and most workers would be idle).
    #[instrument(level = "debug", target = "engine::tree::payload_processor", skip_all)]
    pub fn spawn_state_root<F>(
        &self,
        multiproof_provider_factory: F,
        parent_state_root: B256,
        halve_workers: bool,
        config: &TreeConfig,
    ) -> StateRootHandle
    where
        F: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let (updates_tx, from_multi_proof) = crossbeam_channel::unbounded();

        let task_ctx = ProofTaskCtx::new(multiproof_provider_factory);
        #[cfg(feature = "trie-debug")]
        let task_ctx = task_ctx.with_proof_jitter(config.proof_jitter());
        let proof_handle = ProofWorkerHandle::new(&self.executor, task_ctx, halve_workers);

        let (state_root_tx, state_root_rx) = channel();

        self.spawn_sparse_trie_task(
            proof_handle,
            state_root_tx,
            from_multi_proof,
            parent_state_root,
            config.multiproof_chunk_size(),
        );

        StateRootHandle::new(parent_state_root, updates_tx, state_root_rx)
    }

    /// Transaction count threshold below which proof workers are halved, since fewer transactions
    /// produce fewer state changes and most workers would be idle overhead.
    const SMALL_BLOCK_PROOF_WORKER_TX_THRESHOLD: usize = 30;

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

    /// Spawns a task advancing transaction env iterator and streaming updates through a channel.
    ///
    /// For blocks with fewer than [`Self::SMALL_BLOCK_TX_THRESHOLD`] transactions, uses
    /// sequential iteration to avoid rayon overhead. For larger blocks, uses rayon parallel
    /// iteration with [`ForEachOrdered`] to convert transactions in parallel while streaming
    /// results to execution in the original transaction order.
    #[expect(clippy::type_complexity)]
    #[instrument(level = "debug", target = "engine::tree::payload_processor", skip_all)]
    fn spawn_tx_iterator<I: ExecutableTxIterator<Evm>>(
        &self,
        transactions: I,
        transaction_count: usize,
    ) -> (
        mpsc::Receiver<(usize, WithTxEnv<TxEnvFor<Evm>, I::Recovered>)>,
        mpsc::Receiver<Result<WithTxEnv<TxEnvFor<Evm>, I::Recovered>, I::Error>>,
    ) {
        let (prewarm_tx, prewarm_rx) = mpsc::sync_channel(transaction_count);
        let (execute_tx, execute_rx) = mpsc::sync_channel(transaction_count);

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
            // to execution in order via `for_each_ordered`.
            //
            // To avoid a ~1ms stall waiting for rayon to schedule index 0, the first
            // few transactions are recovered sequentially and sent immediately before
            // entering the parallel iterator for the remainder.
            let prefetch = Self::PARALLEL_PREFETCH_COUNT.min(transaction_count);
            let executor = self.executor.clone();
            self.executor.spawn_blocking_named("tx-iterator", move || {
                let (transactions, convert) = transactions.into_parts();
                let mut all: Vec<_> = transactions.into_iter().collect();
                let rest = all.split_off(prefetch.min(all.len()));

                // Convert the first few transactions sequentially so execution can
                // start immediately without waiting for rayon work-stealing.
                convert_serial(all.into_iter(), &convert, &prewarm_tx, &execute_tx);

                // Convert the remaining transactions in parallel.
                rest.into_par_iter()
                    .enumerate()
                    .map(|(i, tx)| {
                        let idx = i + prefetch;
                        let tx = convert.convert(tx);
                        (idx, tx)
                    })
                    .for_each_ordered_in(executor.cpu_pool(), |(idx, tx)| {
                        let tx = tx.map(|tx| {
                            let (tx_env, tx) = tx.into_parts();
                            let tx = WithTxEnv { tx_env, tx: Arc::new(tx) };
                            let _ = prewarm_tx.send((idx, tx.clone()));
                            tx
                        });
                        let _ = execute_tx.send(tx);
                        trace!(target: "engine::tree::payload_processor", idx, "yielded transaction");
                        });
                        });
        }

        (prewarm_rx, execute_rx)
    }

    /// Spawn prewarming optionally wired to the sparse trie task for target updates.
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor",
        skip_all,
        fields(bal=%env.decoded_bal.is_some())
    )]
    fn spawn_caching_with<P>(
        &self,
        env: ExecutionEnv<Evm>,
        transactions: mpsc::Receiver<(usize, impl ExecutableTxFor<Evm> + Clone + Send + 'static)>,
        provider_builder: StateProviderBuilder<N, P>,
        to_sparse_trie_task: Option<CrossbeamSender<StateRootMessage>>,
    ) -> CacheTaskHandle<N::Receipt>
    where
        P: BlockReader + StateProviderFactory + StateReader + Clone + 'static,
    {
        let skip_prewarm =
            self.disable_transaction_prewarming || env.transaction_count < SMALL_BLOCK_TX_THRESHOLD;

        let saved_cache = self.disable_state_cache.not().then(|| self.cache_for(env.parent_hash));

        let executed_tx_index = Arc::new(AtomicUsize::new(0));
        let maybe_decoded_bal = env.decoded_bal.clone();
        // configure prewarming
        let prewarm_ctx = PrewarmContext {
            env,
            evm_config: self.evm_config.clone(),
            saved_cache: saved_cache.clone(),
            provider: provider_builder,
            metrics: PrewarmMetrics::default(),
            cache_metrics: self.cache_metrics.clone(),
            terminate_execution: Arc::new(AtomicBool::new(false)),
            executed_tx_index: Arc::clone(&executed_tx_index),
            precompile_cache_disabled: self.precompile_cache_disabled,
            precompile_cache_map: self.precompile_cache_map.clone(),
            disable_bal_parallel_state_root: self.disable_bal_parallel_state_root,
            disable_bal_batch_io: self.disable_bal_batch_io,
        };

        let (prewarm_task, to_prewarm_task) = PrewarmCacheTask::new(
            self.executor.clone(),
            self.execution_cache.clone(),
            prewarm_ctx,
            to_sparse_trie_task,
        );
        {
            let to_prewarm_task = to_prewarm_task.clone();
            let disable_bal_parallel_execution = self.disable_bal_parallel_execution;
            self.executor.spawn_blocking_named("prewarm", move || {
                let mode = if skip_prewarm {
                    PrewarmMode::Skipped
                } else if let Some(decoded_bal) =
                    maybe_decoded_bal.filter(|_| !disable_bal_parallel_execution)
                {
                    PrewarmMode::BlockAccessList(decoded_bal)
                } else {
                    PrewarmMode::Transactions(transactions)
                };
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

    /// Spawns the [`SparseTrieCacheTask`] for this payload processor.
    ///
    /// The trie is preserved when the new payload is a child of the previous one.
    fn spawn_sparse_trie_task(
        &self,
        proof_worker_handle: ProofWorkerHandle,
        state_root_tx: mpsc::Sender<Result<StateRootComputeOutcome, ParallelStateRootError>>,
        from_multi_proof: CrossbeamReceiver<StateRootMessage>,
        parent_state_root: B256,
        chunk_size: usize,
    ) {
        let preserved_sparse_trie = self.sparse_state_trie.clone();
        let trie_metrics = self.trie_metrics.clone();
        let max_hot_slots = self.sparse_trie_max_hot_slots;
        let max_hot_accounts = self.sparse_trie_max_hot_accounts;
        let disable_cache_pruning = self.disable_sparse_trie_cache_pruning;
        let executor = self.executor.clone();

        let parent_span = Span::current();
        self.executor.spawn_blocking_named("sparse-trie", move || {
            reth_tasks::once!(increase_thread_priority);

            let _enter = debug_span!(target: "engine::tree::payload_processor", parent: parent_span, "sparse_trie_task")
                .entered();

            // Reuse a stored SparseStateTrie if available, applying continuation logic.
            // If this payload's parent state root matches the preserved trie's anchor,
            // we can reuse the pruned trie structure. Otherwise, we clear the trie but
            // keep allocations.
            let start = Instant::now();
            let preserved = preserved_sparse_trie.take();
            trie_metrics
                .sparse_trie_cache_wait_duration_histogram
                .record(start.elapsed().as_secs_f64());

            let mut sparse_state_trie = preserved
                .map(|preserved| preserved.into_trie_for(parent_state_root))
                .unwrap_or_else(|| {
                    debug!(
                        target: "engine::tree::payload_processor",
                        "Creating new sparse trie - no preserved trie available"
                    );
                    let default_trie = RevealableSparseTrie::blind_from(
                        ConfigurableSparseTrie::Arena(ArenaParallelSparseTrie::default()),
                    );
                    SparseStateTrie::default()
                        .with_accounts_trie(default_trie.clone())
                        .with_default_storage_trie(default_trie)
                        .with_updates(true)
                });
            sparse_state_trie.set_hot_cache_capacities(max_hot_slots, max_hot_accounts);

            let mut task = SparseTrieCacheTask::new_with_trie(
                &executor,
                from_multi_proof,
                proof_worker_handle,
                trie_metrics.clone(),
                sparse_state_trie,
                parent_state_root,
                chunk_size,
            );

            let result = task.run();

            // Acquire the guard before sending the result to prevent a race condition:
            // Without this, the next block could start after send() but before store(),
            // causing take() to return None and forcing it to create a new empty trie
            // instead of reusing the preserved one. Holding the guard ensures the next
            // block's take() blocks until we've stored the trie for reuse.
            let mut guard = preserved_sparse_trie.lock();

            let task_result = result.as_ref().ok().cloned();
            // Send state root computation result - next block may start but will block on take()
            if state_root_tx.send(result).is_err() {
                // Receiver dropped - payload was likely invalid or cancelled.
                // Clear the trie instead of preserving potentially invalid state.
                debug!(
                    target: "engine::tree::payload_processor",
                    "State root receiver dropped, clearing trie"
                );
                let (trie, deferred) = task.into_cleared_trie(
                    SPARSE_TRIE_MAX_NODES_SHRINK_CAPACITY,
                    SPARSE_TRIE_MAX_VALUES_SHRINK_CAPACITY,
                );
                guard.store(PreservedSparseTrie::cleared(trie));
                drop(guard);
                executor.spawn_drop(deferred);
                return;
            }

            // Only preserve the trie as anchored if computation succeeded.
            // A failed computation may have left the trie in a partially updated state.
            let _enter =
                debug_span!(target: "engine::tree::payload_processor", "preserve").entered();
            let deferred = if let Some(result) = task_result {
                let start = Instant::now();
                let (trie, deferred) = task.into_trie_for_reuse(
                    max_hot_slots,
                    max_hot_accounts,
                    SPARSE_TRIE_MAX_NODES_SHRINK_CAPACITY,
                    SPARSE_TRIE_MAX_VALUES_SHRINK_CAPACITY,
                    disable_cache_pruning,
                    &result.trie_updates,
                );
                trie_metrics
                    .into_trie_for_reuse_duration_histogram
                    .record(start.elapsed().as_secs_f64());
                trie_metrics
                    .sparse_trie_retained_memory_bytes
                    .set(trie.memory_size() as f64);
                trie_metrics
                    .sparse_trie_retained_storage_tries
                    .set(trie.retained_storage_tries_count() as f64);
                guard.store(PreservedSparseTrie::anchored(trie, result.state_root));
                deferred
            } else {
                debug!(
                    target: "engine::tree::payload_processor",
                    "State root computation failed, clearing trie"
                );
                let (trie, deferred) = task.into_cleared_trie(
                    SPARSE_TRIE_MAX_NODES_SHRINK_CAPACITY,
                    SPARSE_TRIE_MAX_VALUES_SHRINK_CAPACITY,
                );
                guard.store(PreservedSparseTrie::cleared(trie));
                deferred
            };
            drop(guard);
            executor.spawn_drop(deferred);
        });
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
        bundle_state: &BundleState,
    ) {
        let cache_metrics = self.cache_metrics.clone();
        self.execution_cache.update_with_guard(|cached| {
            if cached.as_ref().is_some_and(|c| c.executed_block_hash() != block_with_parent.parent) {
                debug!(
                    target: "engine::caching",
                    parent_hash = %block_with_parent.parent,
                    "Cannot find cache for parent hash, skip updating cache with new state for inserted executed block",
                );
                return
            }

            // Take existing cache (if any) or create fresh caches
            let caches = match cached.take() {
                Some(existing) => existing.cache().clone(),
                None => ExecutionCache::new(self.cross_block_cache_size),
            };

            // Insert the block's bundle state into cache
            let new_cache = SavedCache::new(block_with_parent.block.hash, caches);
            if new_cache.cache().insert_state(bundle_state).is_err() {
                *cached = None;
                debug!(target: "engine::caching", "cleared execution cache on update error");
                return
            }
            new_cache.update_metrics(cache_metrics.as_ref());

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
    execute_tx: &mpsc::SyncSender<Result<WithTxEnv<TxEnv, Recovered>, Err>>,
) where
    Tx: ExecutableTxParts<TxEnv, InnerTx, Recovered = Recovered>,
    TxEnv: Clone,
    C: ConvertTx<RawTx, Tx = Tx, Error = Err>,
{
    for (idx, raw_tx) in iter.enumerate() {
        let tx = convert.convert(raw_tx);
        let tx = tx.map(|tx| {
            let (tx_env, tx) = tx.into_parts();
            WithTxEnv { tx_env, tx: Arc::new(tx) }
        });
        if let Ok(tx) = &tx {
            let _ = prewarm_tx.send((idx, tx.clone()));
        }
        let _ = execute_tx.send(tx);
        trace!(target: "engine::tree::payload_processor", idx, "yielded transaction");
    }
}

/// Handle to all the spawned tasks.
///
/// Generic over `R` (receipt type) to allow sharing `Arc<ExecutionOutcome<R>>` with the
/// caching task without cloning the expensive `BundleState`.
#[derive(Debug)]
pub struct PayloadHandle<Tx, Err, R> {
    /// Handle to the background state root computation, if spawned.
    state_root_handle: Option<StateRootHandle>,
    /// Whether main execution should stream per-tx state updates into the sparse trie task.
    install_state_hook: bool,
    // must include the receiver of the state root wired to the sparse trie
    prewarm_handle: CacheTaskHandle<R>,
    /// Stream of block transactions
    transactions: mpsc::Receiver<Result<Tx, Err>>,
    /// Span for tracing
    _span: Span,
}

impl<Tx, Err, R: Send + Sync + 'static> PayloadHandle<Tx, Err, R> {
    /// Awaits the state root
    ///
    /// # Panics
    ///
    /// If payload processing was started without background tasks.
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor",
        name = "await_state_root",
        skip_all
    )]
    pub fn state_root(&mut self) -> Result<StateRootComputeOutcome, ParallelStateRootError> {
        self.state_root_handle.as_mut().expect("state_root_handle is None").state_root()
    }

    /// Takes the state root receiver out of the handle for use with custom waiting logic
    /// (e.g., timeout-based waiting).
    ///
    /// # Panics
    ///
    /// If payload processing was started without background tasks.
    pub const fn take_state_root_rx(
        &mut self,
    ) -> mpsc::Receiver<Result<StateRootComputeOutcome, ParallelStateRootError>> {
        self.state_root_handle.as_mut().expect("state_root_handle is None").take_state_root_rx()
    }

    /// Returns a state hook to stream execution state updates to the sparse trie cache task.
    ///
    /// Returns `None` when execution should not send state updates, such as BAL-driven execution.
    pub fn state_hook(&self) -> Option<impl OnStateHook> {
        self.install_state_hook
            .then(|| self.state_root_handle.as_ref().map(|handle| handle.state_hook()))
            .flatten()
    }

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
    /// path without cloning the expensive `BundleState`.
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
        self.transactions.iter()
    }
}

/// Access to the spawned [`PrewarmCacheTask`].
///
/// Generic over `R` (receipt type) to allow sharing `Arc<ExecutionOutcome<R>>` with the
/// prewarm task without cloning the expensive `BundleState`.
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
    /// bundle state. Using `Arc<ExecutionOutcome>` avoids cloning the expensive `BundleState`.
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

/// EVM context required to execute a block.
#[derive(Debug, Clone)]
pub struct ExecutionEnv<Evm: ConfigureEvm> {
    /// Evm environment.
    pub evm_env: EvmEnvFor<Evm>,
    /// Hash of the block being executed.
    pub hash: B256,
    /// Hash of the parent block.
    pub parent_hash: B256,
    /// State root of the parent block.
    /// Used for sparse trie continuation: if the preserved trie's anchor matches this,
    /// the trie can be reused directly.
    pub parent_state_root: B256,
    /// Number of transactions in the block.
    /// Used to determine parallel worker count for prewarming.
    /// A value of 0 indicates the count is unknown.
    pub transaction_count: usize,
    /// Total gas used by all transactions in the block.
    /// Used to adaptively select multiproof chunk size for optimal throughput.
    pub gas_used: u64,
    /// Withdrawals included in the block.
    /// Used to generate prefetch targets for withdrawal addresses.
    pub withdrawals: Option<Vec<Withdrawal>>,
    /// Optional decoded BAL for the block.
    /// Used to validate and optimize execution.
    pub decoded_bal: Option<Arc<DecodedBal>>,
}

impl<Evm: ConfigureEvm> ExecutionEnv<Evm>
where
    EvmEnvFor<Evm>: Default,
{
    /// Creates a new [`ExecutionEnv`] with default values for testing.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn test_default() -> Self {
        Self {
            evm_env: Default::default(),
            hash: Default::default(),
            parent_hash: Default::default(),
            parent_state_root: Default::default(),
            transaction_count: 0,
            gas_used: 0,
            withdrawals: None,
            decoded_bal: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::tree::{
        payload_processor::{evm_state_to_hashed_post_state, ExecutionEnv, PayloadProcessor},
        precompile_cache::PrecompileCacheMap,
        ExecutionCache, PayloadExecutionCache, SavedCache, StateProviderBuilder, TreeConfig,
    };
    use alloy_eips::eip1898::{BlockNumHash, BlockWithParent};
    use alloy_evm::block::StateChangeSource;
    use rand::Rng;
    use reth_chainspec::ChainSpec;
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
    use reth_evm::OnStateHook;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::{Account, Recovered, StorageEntry};
    use reth_provider::{
        providers::{BlockchainProvider, OverlayBuilder, OverlayStateProviderFactory},
        test_utils::create_test_provider_factory_with_chain_spec,
        ChainSpecProvider, HashingWriter,
    };
    use reth_revm::db::BundleState;
    use reth_testing_utils::generators;
    use reth_trie::{test_utils::state_root, HashedPostState};
    use reth_trie_db::ChangesetCache;
    use revm_primitives::{Address, HashMap, B256, KECCAK_EMPTY, U256};
    use revm_state::{AccountInfo, AccountStatus, EvmState, EvmStorageSlot};
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
            PrecompileCacheMap::default(),
        );

        let parent_hash = B256::from([1u8; 32]);
        let block_hash = B256::from([10u8; 32]);
        let block_with_parent = BlockWithParent {
            block: BlockNumHash { hash: block_hash, number: 1 },
            parent: parent_hash,
        };
        let bundle_state = BundleState::default();

        // Cache should be empty initially
        assert!(payload_processor.execution_cache.get_cache_for(block_hash).is_none());

        // Update cache with inserted block
        payload_processor.on_inserted_executed_block(block_with_parent, &bundle_state);

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
            PrecompileCacheMap::default(),
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
        let bundle_state = BundleState::default();

        payload_processor.on_inserted_executed_block(block_with_parent, &bundle_state);

        // Cache should still be for block 1 (unchanged)
        let cached = payload_processor.execution_cache.get_cache_for(block1_hash);
        assert!(cached.is_some(), "Original cache should be preserved");

        // Cache for block 3 should not exist
        let cached3 = payload_processor.execution_cache.get_cache_for(block3_hash);
        assert!(cached3.is_none(), "New block cache should not be created on mismatch");
    }

    fn create_mock_state_updates(num_accounts: usize, updates_per_account: usize) -> Vec<EvmState> {
        let mut rng = generators::rng();
        let all_addresses: Vec<Address> = (0..num_accounts).map(|_| rng.random()).collect();
        let mut updates = Vec::with_capacity(updates_per_account);

        for _ in 0..updates_per_account {
            let num_accounts_in_update = rng.random_range(1..=num_accounts);
            let mut state_update = EvmState::default();

            let selected_addresses = &all_addresses[0..num_accounts_in_update];

            for &address in selected_addresses {
                let mut storage = HashMap::default();
                if rng.random_bool(0.7) {
                    for _ in 0..rng.random_range(1..10) {
                        let slot = U256::from(rng.random::<u64>());
                        storage.insert(
                            slot,
                            EvmStorageSlot::new_changed(
                                U256::ZERO,
                                U256::from(rng.random::<u64>()),
                                0,
                            ),
                        );
                    }
                }

                let mut account = revm_state::Account::default();
                account.info = AccountInfo {
                    balance: U256::from(rng.random::<u64>()),
                    nonce: rng.random::<u64>(),
                    code_hash: KECCAK_EMPTY,
                    code: Some(Default::default()),
                    account_id: None,
                };
                account.storage = storage;
                account.status = AccountStatus::Touched;

                state_update.insert(address, account);
            }

            updates.push(state_update);
        }

        updates
    }

    #[test]
    fn test_state_root() {
        reth_tracing::init_test_tracing();

        let factory = create_test_provider_factory_with_chain_spec(Arc::new(ChainSpec::default()));
        let genesis_hash = init_genesis(&factory).unwrap();

        let state_updates = create_mock_state_updates(10, 10);
        let mut hashed_state = HashedPostState::default();
        let mut accumulated_state: HashMap<Address, (Account, HashMap<B256, U256>)> =
            HashMap::default();

        {
            let provider_rw = factory.provider_rw().expect("failed to get provider");

            for update in &state_updates {
                let account_updates = update.iter().map(|(address, account)| {
                    (*address, Some(Account::from_revm_account(account)))
                });
                provider_rw
                    .insert_account_for_hashing(account_updates)
                    .expect("failed to insert accounts");

                let storage_updates = update.iter().map(|(address, account)| {
                    let storage_entries = account.storage.iter().map(|(slot, value)| {
                        StorageEntry { key: B256::from(*slot), value: value.present_value }
                    });
                    (*address, storage_entries)
                });
                provider_rw
                    .insert_storage_for_hashing(storage_updates)
                    .expect("failed to insert storage");
            }
            provider_rw.commit().expect("failed to commit changes");
        }

        for update in &state_updates {
            hashed_state.extend(evm_state_to_hashed_post_state(update.clone()));

            for (address, account) in update {
                let storage: HashMap<B256, U256> = account
                    .storage
                    .iter()
                    .map(|(k, v)| (B256::from(*k), v.present_value))
                    .collect();

                let entry = accumulated_state.entry(*address).or_default();
                entry.0 = Account::from_revm_account(account);
                entry.1.extend(storage);
            }
        }

        let mut payload_processor = PayloadProcessor::new(
            reth_tasks::Runtime::test(),
            EthEvmConfig::new(factory.chain_spec()),
            &TreeConfig::default(),
            PrecompileCacheMap::default(),
        );

        let provider_factory = BlockchainProvider::new(factory).unwrap();

        let mut handle = payload_processor.spawn(
            ExecutionEnv::test_default(),
            (
                Vec::<Result<Recovered<TransactionSigned>, core::convert::Infallible>>::new(),
                std::convert::identity,
            ),
            StateProviderBuilder::new(provider_factory.clone(), genesis_hash, None),
            OverlayStateProviderFactory::new(
                provider_factory,
                OverlayBuilder::<EthPrimitives>::new(genesis_hash, ChangesetCache::new()),
            ),
            &TreeConfig::default(),
        );

        let mut state_hook = handle.state_hook().expect("state hook is None");

        for (i, update) in state_updates.into_iter().enumerate() {
            state_hook.on_state(StateChangeSource::Transaction(i), &update);
        }
        drop(state_hook);

        let root_from_task = handle.state_root().expect("task failed").state_root;
        let root_from_regular = state_root(accumulated_state);

        assert_eq!(
            root_from_task, root_from_regular,
            "State root mismatch: task={root_from_task}, base={root_from_regular}"
        );
    }

    /// Tests the full prewarm lifecycle for a fork block:
    ///
    /// 1. Cache is at canonical block 4.
    /// 2. Fork block (parent = block 2) checks out the cache via `get_cache_for`, simulating what
    ///    `PrewarmCacheTask` does when it receives a `SavedCache`.
    /// 3. Prewarm populates the shared cache with fork-specific state.
    /// 4. While the prewarm clone is alive, the cache is unavailable (`usage_guard` > 1).
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

        // While prewarm holds the clone, the usage_guard count > 1 → cache is in use.
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
