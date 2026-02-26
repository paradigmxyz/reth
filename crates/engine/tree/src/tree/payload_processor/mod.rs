//! Entrypoint for payload processing.

use super::precompile_cache::PrecompileCacheMap;
use crate::tree::{
    cached_state::{CachedStateMetrics, ExecutionCache, SavedCache},
    payload_processor::{
        prewarm::{PrewarmCacheTask, PrewarmContext, PrewarmMode, PrewarmTaskEvent},
        sparse_trie::StateRootComputeOutcome,
    },
    sparse_trie::SparseTrieCacheTask,
    CacheWaitDurations, StateProviderBuilder, TreeConfig, WaitForCaches,
};
use alloy_eip7928::BlockAccessList;
use alloy_eips::{eip1898::BlockWithParent, eip4895::Withdrawal};
use alloy_evm::block::StateChangeSource;
use alloy_primitives::B256;
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use metrics::{Counter, Histogram};
use multiproof::*;
use parking_lot::RwLock;
use prewarm::PrewarmMetrics;
use rayon::prelude::*;
use reth_evm::{
    block::ExecutableTxParts,
    execute::{ExecutableTxFor, WithTxEnv},
    ConfigureEvm, ConvertTx, EvmEnvFor, ExecutableTxIterator, ExecutableTxTuple, OnStateHook,
    SpecFor, TxEnvFor,
};
use reth_metrics::Metrics;
use reth_primitives_traits::{FastInstant as Instant, NodePrimitives};
use reth_provider::{
    BlockExecutionOutput, BlockReader, DatabaseProviderROFactory, StateProviderFactory, StateReader,
};
use reth_revm::{db::BundleState, state::EvmState};
use reth_tasks::{utils::increase_thread_priority, ForEachOrdered, Runtime};
use reth_trie::{hashed_cursor::HashedCursorFactory, trie_cursor::TrieCursorFactory};
use reth_trie_parallel::{
    proof_task::{ProofTaskCtx, ProofWorkerHandle},
    root::ParallelStateRootError,
};
use reth_trie_sparse::{
    ParallelSparseTrie, ParallelismThresholds, RevealableSparseTrie, SparseStateTrie,
};
use std::{
    ops::Not,
    sync::{
        atomic::AtomicBool,
        mpsc::{self, channel},
        Arc,
    },
    time::Duration,
};
use tracing::{debug, debug_span, instrument, warn, Span};

pub mod bal;
pub mod multiproof;
mod preserved_sparse_trie;
pub mod prewarm;
pub mod receipt_root_task;
pub mod sparse_trie;

use preserved_sparse_trie::{PreservedSparseTrie, SharedPreservedSparseTrie};

/// Default parallelism thresholds to use with the [`ParallelSparseTrie`].
///
/// These values were determined by performing benchmarks using gradually increasing values to judge
/// the affects. Below 100 throughput would generally be equal or slightly less, while above 150 it
/// would deteriorate to the point where PST might as well not be used.
pub const PARALLEL_SPARSE_TRIE_PARALLELISM_THRESHOLDS: ParallelismThresholds =
    ParallelismThresholds { min_revealed_nodes: 100, min_updated_nodes: 100 };

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
    /// Sparse trie prune depth.
    sparse_trie_prune_depth: usize,
    /// Maximum storage tries to retain after pruning.
    sparse_trie_max_storage_tries: usize,
    /// Whether sparse trie cache pruning is fully disabled.
    disable_sparse_trie_cache_pruning: bool,
    /// Whether to disable cache metrics recording.
    disable_cache_metrics: bool,
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
            sparse_trie_prune_depth: config.sparse_trie_prune_depth(),
            sparse_trie_max_storage_tries: config.sparse_trie_max_storage_tries(),
            disable_sparse_trie_cache_pruning: config.disable_sparse_trie_cache_pruning(),
            disable_cache_metrics: config.disable_cache_metrics(),
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

        self.executor.spawn_blocking(move || {
            let _ = execution_tx.send(execution_cache.wait_for_availability());
        });
        self.executor.spawn_blocking(move || {
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
    /// Responsible for feeding state updates to the multi proof task.
    ///
    /// This task runs until:
    ///  - externally cancelled (e.g. sequential block execution is complete)
    ///
    /// ## Multi proof task
    ///
    /// Responsible for preparing sparse trie messages for the sparse trie task.
    /// A state update (e.g. tx output) is converted into a multiproof calculation that returns an
    /// output back to this task.
    ///
    /// Receives updates from sequential execution.
    /// This task runs until it receives a shutdown signal, which should be after the block
    /// was fully executed.
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
        bal: Option<Arc<BlockAccessList>>,
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
        let (to_multi_proof, from_multi_proof) = crossbeam_channel::unbounded();

        let parent_state_root = env.parent_state_root;
        let transaction_count = env.transaction_count;
        let chunk_size = config.multiproof_chunk_size();
        let prewarm_handle = self.spawn_caching_with(
            env,
            prewarm_rx,
            provider_builder,
            Some(to_multi_proof.clone()),
            bal,
        );

        // Create and spawn the storage proof task.
        let task_ctx = ProofTaskCtx::new(multiproof_provider_factory);
        let halve_workers = transaction_count <= Self::SMALL_BLOCK_PROOF_WORKER_TX_THRESHOLD;
        let proof_handle = ProofWorkerHandle::new(&self.executor, task_ctx, halve_workers);

        // wire the sparse trie to the state root response receiver
        let (state_root_tx, state_root_rx) = channel();

        // Spawn the sparse trie task using any stored trie and parallel trie configuration.
        self.spawn_sparse_trie_task(
            proof_handle,
            state_root_tx,
            from_multi_proof,
            parent_state_root,
            chunk_size,
        );

        PayloadHandle {
            to_multi_proof: Some(to_multi_proof),
            prewarm_handle,
            state_root: Some(state_root_rx),
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
        bal: Option<Arc<BlockAccessList>>,
    ) -> IteratorPayloadHandle<Evm, I, N>
    where
        P: BlockReader + StateProviderFactory + StateReader + Clone + 'static,
    {
        let (prewarm_rx, execution_rx) =
            self.spawn_tx_iterator(transactions, env.transaction_count);
        let prewarm_handle = self.spawn_caching_with(env, prewarm_rx, provider_builder, None, bal);
        PayloadHandle {
            to_multi_proof: None,
            prewarm_handle,
            state_root: None,
            transactions: execution_rx,
            _span: Span::current(),
        }
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
                        let tx = tx.map(|tx| {
                            let (tx_env, tx) = tx.into_parts();
                            let tx = WithTxEnv { tx_env, tx: Arc::new(tx) };
                            let _ = prewarm_tx.send((idx, tx.clone()));
                            tx
                        });
                        (idx, tx)
                    })
                    .for_each_ordered(|(idx, tx)| {
                        let _ = execute_tx.send(tx);
                        debug!(target: "engine::tree::payload_processor", idx, "yielded transaction");
                    });
            });
        }

        (prewarm_rx, execute_rx)
    }

    /// Spawn prewarming optionally wired to the multiproof task for target updates.
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor",
        skip_all,
        fields(bal=%bal.is_some())
    )]
    fn spawn_caching_with<P>(
        &self,
        env: ExecutionEnv<Evm>,
        transactions: mpsc::Receiver<(usize, impl ExecutableTxFor<Evm> + Clone + Send + 'static)>,
        provider_builder: StateProviderBuilder<N, P>,
        to_multi_proof: Option<CrossbeamSender<MultiProofMessage>>,
        bal: Option<Arc<BlockAccessList>>,
    ) -> CacheTaskHandle<N::Receipt>
    where
        P: BlockReader + StateProviderFactory + StateReader + Clone + 'static,
    {
        let skip_prewarm =
            self.disable_transaction_prewarming || env.transaction_count < SMALL_BLOCK_TX_THRESHOLD;

        let saved_cache = self.disable_state_cache.not().then(|| self.cache_for(env.parent_hash));

        // configure prewarming
        let prewarm_ctx = PrewarmContext {
            env,
            evm_config: self.evm_config.clone(),
            saved_cache: saved_cache.clone(),
            provider: provider_builder,
            metrics: PrewarmMetrics::default(),
            terminate_execution: Arc::new(AtomicBool::new(false)),
            precompile_cache_disabled: self.precompile_cache_disabled,
            precompile_cache_map: self.precompile_cache_map.clone(),
        };

        let (prewarm_task, to_prewarm_task) = PrewarmCacheTask::new(
            self.executor.clone(),
            self.execution_cache.clone(),
            prewarm_ctx,
            to_multi_proof,
        );

        {
            let to_prewarm_task = to_prewarm_task.clone();
            self.executor.spawn_blocking_named("prewarm", move || {
                let mode = if skip_prewarm {
                    PrewarmMode::Skipped
                } else if let Some(bal) = bal {
                    PrewarmMode::BlockAccessList(bal)
                } else {
                    PrewarmMode::Transactions(transactions)
                };
                prewarm_task.run(mode, to_prewarm_task);
            });
        }

        CacheTaskHandle { saved_cache, to_prewarm_task: Some(to_prewarm_task) }
    }

    /// Returns the cache for the given parent hash.
    ///
    /// If the given hash is different then what is recently cached, then this will create a new
    /// instance.
    #[instrument(level = "debug", target = "engine::caching", skip(self))]
    fn cache_for(&self, parent_hash: B256) -> SavedCache {
        if let Some(cache) = self.execution_cache.get_cache_for(parent_hash) {
            debug!("reusing execution cache");
            cache
        } else {
            debug!("creating new execution cache on cache miss");
            let start = Instant::now();
            let cache = ExecutionCache::new(self.cross_block_cache_size);
            let metrics = CachedStateMetrics::zeroed();
            metrics.record_cache_creation(start.elapsed());
            SavedCache::new(parent_hash, cache, metrics)
                .with_disable_cache_metrics(self.disable_cache_metrics)
        }
    }

    /// Spawns the [`SparseTrieCacheTask`] for this payload processor.
    ///
    /// The trie is preserved when the new payload is a child of the previous one.
    fn spawn_sparse_trie_task(
        &self,
        proof_worker_handle: ProofWorkerHandle,
        state_root_tx: mpsc::Sender<Result<StateRootComputeOutcome, ParallelStateRootError>>,
        from_multi_proof: CrossbeamReceiver<MultiProofMessage>,
        parent_state_root: B256,
        chunk_size: usize,
    ) {
        let preserved_sparse_trie = self.sparse_state_trie.clone();
        let trie_metrics = self.trie_metrics.clone();
        let prune_depth = self.sparse_trie_prune_depth;
        let max_storage_tries = self.sparse_trie_max_storage_tries;
        let disable_cache_pruning = self.disable_sparse_trie_cache_pruning;
        let executor = self.executor.clone();

        let parent_span = Span::current();
        self.executor.spawn_blocking_named("sparse-trie", move || {
            increase_thread_priority();

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

            let sparse_state_trie = preserved
                .map(|preserved| preserved.into_trie_for(parent_state_root))
                .unwrap_or_else(|| {
                    debug!(
                        target: "engine::tree::payload_processor",
                        "Creating new sparse trie - no preserved trie available"
                    );
                    let default_trie = RevealableSparseTrie::blind_from(
                        ParallelSparseTrie::default().with_parallelism_thresholds(
                            PARALLEL_SPARSE_TRIE_PARALLELISM_THRESHOLDS,
                        ),
                    );
                    SparseStateTrie::new()
                        .with_accounts_trie(default_trie.clone())
                        .with_default_storage_trie(default_trie)
                        .with_updates(true)
                });

            let mut task = SparseTrieCacheTask::new_with_trie(
                &executor,
                from_multi_proof,
                proof_worker_handle,
                trie_metrics.clone(),
                sparse_state_trie.with_skip_proof_node_filtering(true),
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
                // Drop guard before deferred to release lock before expensive deallocations
                drop(guard);
                drop(deferred);
                return;
            }

            // Only preserve the trie as anchored if computation succeeded.
            // A failed computation may have left the trie in a partially updated state.
            let _enter =
                debug_span!(target: "engine::tree::payload_processor", "preserve").entered();
            let deferred = if let Some(result) = task_result {
                let start = Instant::now();
                let (trie, deferred) = task.into_trie_for_reuse(
                    prune_depth,
                    max_storage_tries,
                    SPARSE_TRIE_MAX_NODES_SHRINK_CAPACITY,
                    SPARSE_TRIE_MAX_VALUES_SHRINK_CAPACITY,
                    disable_cache_pruning,
                    &result.trie_updates,
                );
                trie_metrics
                    .into_trie_for_reuse_duration_histogram
                    .record(start.elapsed().as_secs_f64());
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
            // Drop guard before deferred to release lock before expensive deallocations
            drop(guard);
            drop(deferred);
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
        let disable_cache_metrics = self.disable_cache_metrics;
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
            let (caches, cache_metrics, _) = match cached.take() {
                Some(existing) => existing.split(),
                None => (
                    ExecutionCache::new(self.cross_block_cache_size),
                    CachedStateMetrics::zeroed(),
                    false,
                ),
            };

            // Insert the block's bundle state into cache
            let new_cache =
                SavedCache::new(block_with_parent.block.hash, caches, cache_metrics)
                    .with_disable_cache_metrics(disable_cache_metrics);
            if new_cache.cache().insert_state(bundle_state).is_err() {
                *cached = None;
                debug!(target: "engine::caching", "cleared execution cache on update error");
                return
            }
            new_cache.update_metrics();

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
        debug!(target: "engine::tree::payload_processor", idx, "yielded transaction");
    }
}

/// Handle to all the spawned tasks.
///
/// Generic over `R` (receipt type) to allow sharing `Arc<ExecutionOutcome<R>>` with the
/// caching task without cloning the expensive `BundleState`.
#[derive(Debug)]
pub struct PayloadHandle<Tx, Err, R> {
    /// Channel for evm state updates
    to_multi_proof: Option<CrossbeamSender<MultiProofMessage>>,
    // must include the receiver of the state root wired to the sparse trie
    prewarm_handle: CacheTaskHandle<R>,
    /// Stream of block transactions
    transactions: mpsc::Receiver<Result<Tx, Err>>,
    /// Receiver for the state root
    state_root: Option<mpsc::Receiver<Result<StateRootComputeOutcome, ParallelStateRootError>>>,
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
        self.state_root
            .take()
            .expect("state_root is None")
            .recv()
            .map_err(|_| ParallelStateRootError::Other("sparse trie task dropped".to_string()))?
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
        self.state_root.take().expect("state_root is None")
    }

    /// Returns a state hook to be used to send state updates to this task.
    ///
    /// If a multiproof task is spawned the hook will notify it about new states.
    pub fn state_hook(&self) -> impl OnStateHook {
        // convert the channel into a `StateHookSender` that emits an event on drop
        let to_multi_proof = self.to_multi_proof.clone().map(StateHookSender::new);

        move |source: StateChangeSource, state: &EvmState| {
            if let Some(sender) = &to_multi_proof {
                let _ = sender.send(MultiProofMessage::StateUpdate(source.into(), state.clone()));
            }
        }
    }

    /// Returns a clone of the caches used by prewarming
    pub fn caches(&self) -> Option<ExecutionCache> {
        self.prewarm_handle.saved_cache.as_ref().map(|cache| cache.cache().clone())
    }

    /// Returns a clone of the cache metrics used by prewarming
    pub fn cache_metrics(&self) -> Option<CachedStateMetrics> {
        self.prewarm_handle.saved_cache.as_ref().map(|cache| cache.metrics().clone())
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
        core::iter::repeat_with(|| self.transactions.recv())
            .take_while(|res| res.is_ok())
            .map(|res| res.unwrap())
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

/// Shared access to most recently used cache.
///
/// This cache is intended to used for processing the payload in the following manner:
///  - Get Cache if the payload's parent block matches the parent block
///  - Update cache upon successful payload execution
///
/// This process assumes that payloads are received sequentially.
///
/// ## Cache Safety
///
/// **CRITICAL**: Cache update operations require exclusive access. All concurrent cache users
/// (such as prewarming tasks) must be terminated before calling
/// [`PayloadExecutionCache::update_with_guard`], otherwise the cache may be corrupted or cleared.
///
/// ## Cache vs Prewarming Distinction
///
/// **[`PayloadExecutionCache`]**:
/// - Stores parent block's execution state after completion
/// - Used to fetch parent data for next block's execution
/// - Must be exclusively accessed during save operations
///
/// **[`PrewarmCacheTask`]**:
/// - Speculatively loads accounts/storage that might be used in transaction execution
/// - Prepares data for state root proof computation
/// - Runs concurrently but must not interfere with cache saves
#[derive(Clone, Debug, Default)]
pub struct PayloadExecutionCache {
    /// Guarded cloneable cache identified by a block hash.
    inner: Arc<RwLock<Option<SavedCache>>>,
    /// Metrics for cache operations.
    metrics: ExecutionCacheMetrics,
}

impl PayloadExecutionCache {
    /// Returns the cache for `parent_hash` if it's available for use.
    ///
    /// A cache is considered available when:
    /// - It exists and matches the requested parent hash
    /// - No other tasks are currently using it (checked via Arc reference count)
    #[instrument(level = "debug", target = "engine::tree::payload_processor", skip(self))]
    pub(crate) fn get_cache_for(&self, parent_hash: B256) -> Option<SavedCache> {
        let start = Instant::now();
        let cache = self.inner.read();

        let elapsed = start.elapsed();
        self.metrics.execution_cache_wait_duration.record(elapsed.as_secs_f64());
        if elapsed.as_millis() > 5 {
            warn!(blocked_for=?elapsed, "Blocked waiting for execution cache mutex");
        }

        if let Some(c) = cache.as_ref() {
            let cached_hash = c.executed_block_hash();
            // Check that the cache hash matches the parent hash of the current block. It won't
            // match in case it's a fork block.
            let hash_matches = cached_hash == parent_hash;
            // Check `is_available()` to ensure no other tasks (e.g., prewarming) currently hold
            // a reference to this cache. We can only reuse it when we have exclusive access.
            let available = c.is_available();
            let usage_count = c.usage_count();

            debug!(
                target: "engine::caching",
                %cached_hash,
                %parent_hash,
                hash_matches,
                available,
                usage_count,
                "Existing cache found"
            );

            if available {
                // If the has is available (no other threads are using it), but has a mismatching
                // parent hash, we can just clear it and keep using without re-creating from
                // scratch.
                if !hash_matches {
                    c.clear();
                }
                return Some(c.clone())
            } else if hash_matches {
                self.metrics.execution_cache_in_use.increment(1);
            }
        } else {
            debug!(target: "engine::caching", %parent_hash, "No cache found");
        }

        None
    }

    /// Clears the tracked cache
    #[expect(unused)]
    pub(crate) fn clear(&self) {
        self.inner.write().take();
    }

    /// Waits until the execution cache becomes available for use.
    ///
    /// This acquires a write lock to ensure exclusive access, then immediately releases it.
    /// This is useful for synchronization before starting payload processing.
    ///
    /// Returns the time spent waiting for the lock.
    pub fn wait_for_availability(&self) -> Duration {
        let start = Instant::now();
        // Acquire write lock to wait for any current holders to finish
        let _guard = self.inner.write();
        let elapsed = start.elapsed();
        if elapsed.as_millis() > 5 {
            debug!(
                target: "engine::tree::payload_processor",
                blocked_for=?elapsed,
                "Waited for execution cache to become available"
            );
        }
        elapsed
    }

    /// Updates the cache with a closure that has exclusive access to the guard.
    /// This ensures that all cache operations happen atomically.
    ///
    /// ## CRITICAL SAFETY REQUIREMENT
    ///
    /// **Before calling this method, you MUST ensure there are no other active cache users.**
    /// This includes:
    /// - No running [`PrewarmCacheTask`] instances that could write to the cache
    /// - No concurrent transactions that might access the cached state
    /// - All prewarming operations must be completed or cancelled
    ///
    /// Violating this requirement can result in cache corruption, incorrect state data,
    /// and potential consensus failures.
    pub fn update_with_guard<F>(&self, update_fn: F)
    where
        F: FnOnce(&mut Option<SavedCache>),
    {
        let mut guard = self.inner.write();
        update_fn(&mut guard);
    }
}

/// Metrics for execution cache operations.
#[derive(Metrics, Clone)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct ExecutionCacheMetrics {
    /// Counter for when the execution cache was unavailable because other threads
    /// (e.g., prewarming) are still using it.
    pub(crate) execution_cache_in_use: Counter,
    /// Time spent waiting for execution cache mutex to become available.
    pub(crate) execution_cache_wait_duration: Histogram,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PayloadExecutionCache;
    use crate::tree::{
        cached_state::{CachedStateMetrics, ExecutionCache, SavedCache},
        payload_processor::{evm_state_to_hashed_post_state, ExecutionEnv, PayloadProcessor},
        precompile_cache::PrecompileCacheMap,
        StateProviderBuilder, TreeConfig,
    };
    use alloy_eips::eip1898::{BlockNumHash, BlockWithParent};
    use alloy_evm::block::StateChangeSource;
    use rand::Rng;
    use reth_chainspec::ChainSpec;
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::TransactionSigned;
    use reth_evm::OnStateHook;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::{Account, Recovered, StorageEntry};
    use reth_provider::{
        providers::{BlockchainProvider, OverlayStateProviderFactory},
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
        SavedCache::new(hash, execution_cache, CachedStateMetrics::zeroed())
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

        // When the parent hash doesn't match, the cache is cleared and returned for reuse
        let different_hash = B256::from([4u8; 32]);
        let cache = execution_cache.get_cache_for(different_hash);
        assert!(cache.is_some(), "cache should be returned for reuse after clearing")
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

                let account = revm_state::Account {
                    info: AccountInfo {
                        balance: U256::from(rng.random::<u64>()),
                        nonce: rng.random::<u64>(),
                        code_hash: KECCAK_EMPTY,
                        code: Some(Default::default()),
                        account_id: None,
                    },
                    original_info: Box::new(AccountInfo::default()),
                    storage,
                    status: AccountStatus::Touched,
                    transaction_id: 0,
                };

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
            OverlayStateProviderFactory::new(provider_factory, ChangesetCache::new()),
            &TreeConfig::default(),
            None, // No BAL for test
        );

        let mut state_hook = handle.state_hook();

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
}
