//! Entrypoint for payload processing.

use super::precompile_cache::PrecompileCacheMap;
use crate::tree::{
    cached_state::{
        CachedStateMetrics, CachedStateProvider, ExecutionCache as StateExecutionCache,
        FixedCacheMetrics, SavedCache,
    },
    payload_processor::{
        prewarm::{PrewarmCacheTask, PrewarmContext, PrewarmMode, PrewarmTaskEvent},
        sparse_trie::StateRootComputeOutcome,
    },
    sparse_trie::SparseTrieTask,
    StateProviderBuilder, TreeConfig,
};
use alloy_eip7928::BlockAccessList;
use alloy_eips::eip1898::BlockWithParent;
use alloy_evm::{block::StateChangeSource, ToTxEnv};
use alloy_primitives::B256;
use crossbeam_channel::Sender as CrossbeamSender;
use executor::WorkloadExecutor;
use multiproof::{SparseTrieUpdate, *};
use parking_lot::RwLock;
use prewarm::PrewarmMetrics;
use rayon::prelude::*;
use reth_evm::{
    execute::{ExecutableTxFor, WithTxEnv},
    ConfigureEvm, EvmEnvFor, ExecutableTxIterator, ExecutableTxTuple, OnStateHook, SpecFor,
    TxEnvFor,
};
use reth_execution_types::ExecutionOutcome;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    BlockReader, DatabaseProviderROFactory, StateProvider, StateProviderFactory, StateReader,
};
use reth_revm::{db::BundleState, state::EvmState};
use reth_trie::{hashed_cursor::HashedCursorFactory, trie_cursor::TrieCursorFactory};
use reth_trie_parallel::{
    proof_task::{ProofTaskCtx, ProofWorkerHandle},
    root::ParallelStateRootError,
};
use reth_trie_sparse::{
    provider::{TrieNodeProvider, TrieNodeProviderFactory},
    ClearedSparseStateTrie, SparseStateTrie, SparseTrie,
};
use reth_trie_sparse_parallel::{ParallelSparseTrie, ParallelismThresholds};
use std::{
    collections::BTreeMap,
    ops::Not,
    sync::{
        atomic::AtomicBool,
        mpsc::{self, channel},
        Arc,
    },
    time::Instant,
};
use tracing::{debug, debug_span, instrument, warn, Span};

pub mod bal;
mod configured_sparse_trie;
pub mod executor;
pub mod multiproof;
pub mod prewarm;
pub mod sparse_trie;

use configured_sparse_trie::ConfiguredSparseTrie;

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

/// Type alias for [`PayloadHandle`] returned by payload processor spawn methods.
type IteratorPayloadHandle<Evm, I, N> = PayloadHandle<
    WithTxEnv<TxEnvFor<Evm>, <I as ExecutableTxTuple>::Tx>,
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
    executor: WorkloadExecutor,
    /// The most recent cache used for execution.
    execution_cache: ExecutionCache,
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
    /// A cleared `SparseStateTrie`, kept around to be reused for the state root computation so
    /// that allocations can be minimized.
    sparse_state_trie: Arc<
        parking_lot::Mutex<
            Option<ClearedSparseStateTrie<ConfiguredSparseTrie, ConfiguredSparseTrie>>,
        >,
    >,
    /// Whether to disable the parallel sparse trie.
    disable_parallel_sparse_trie: bool,
    /// Maximum concurrency for prewarm task.
    prewarm_max_concurrency: usize,
}

impl<N, Evm> PayloadProcessor<Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    /// Returns a reference to the workload executor driving payload tasks.
    pub(super) const fn executor(&self) -> &WorkloadExecutor {
        &self.executor
    }

    /// Creates a new payload processor.
    pub fn new(
        executor: WorkloadExecutor,
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
            sparse_state_trie: Arc::default(),
            disable_parallel_sparse_trie: config.disable_parallel_sparse_trie(),
            prewarm_max_concurrency: config.prewarm_max_concurrency(),
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
    /// Responsible for calculating the state root based on the received [`SparseTrieUpdate`].
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
            + 'static,
    {
        // start preparing transactions immediately
        let (prewarm_rx, execution_rx, transaction_count_hint) =
            self.spawn_tx_iterator(transactions);

        let span = Span::current();
        let (to_sparse_trie, sparse_trie_rx) = channel();
        let (to_multi_proof, from_multi_proof) = crossbeam_channel::unbounded();

        // Handle BAL-based optimization if available
        let prewarm_handle = if let Some(bal) = bal {
            // When BAL is present, use BAL prewarming and send BAL to multiproof
            debug!(target: "engine::tree::payload_processor", "BAL present, using BAL prewarming");

            // Send BAL message immediately to MultiProofTask
            let _ = to_multi_proof.send(MultiProofMessage::BlockAccessList(Arc::clone(&bal)));

            // Spawn with BAL prewarming
            self.spawn_caching_with(
                env,
                prewarm_rx,
                transaction_count_hint,
                provider_builder.clone(),
                None, // Don't send proof targets when BAL is present
                Some(bal),
            )
        } else {
            // Normal path: spawn with transaction prewarming
            self.spawn_caching_with(
                env,
                prewarm_rx,
                transaction_count_hint,
                provider_builder.clone(),
                Some(to_multi_proof.clone()),
                None,
            )
        };

        // Create and spawn the storage proof task
        let task_ctx = ProofTaskCtx::new(multiproof_provider_factory);
        let storage_worker_count = config.storage_worker_count();
        let account_worker_count = config.account_worker_count();
        let v2_proofs_enabled = config.enable_proof_v2();
        let proof_handle = ProofWorkerHandle::new(
            self.executor.handle().clone(),
            task_ctx,
            storage_worker_count,
            account_worker_count,
            v2_proofs_enabled,
        );

        let multi_proof_task = MultiProofTask::new(
            proof_handle.clone(),
            to_sparse_trie,
            config.multiproof_chunking_enabled().then_some(config.multiproof_chunk_size()),
            to_multi_proof.clone(),
            from_multi_proof,
        );

        // spawn multi-proof task
        let parent_span = span.clone();
        let saved_cache = prewarm_handle.saved_cache.clone();
        self.executor.spawn_blocking(move || {
            let _enter = parent_span.entered();
            // Build a state provider for the multiproof task
            let provider = provider_builder.build().expect("failed to build provider");
            let provider = if let Some(saved_cache) = saved_cache {
                let (cache, metrics) = saved_cache.split();
                Box::new(CachedStateProvider::new(provider, cache, metrics))
                    as Box<dyn StateProvider>
            } else {
                Box::new(provider)
            };
            multi_proof_task.run(provider);
        });

        // wire the sparse trie to the state root response receiver
        let (state_root_tx, state_root_rx) = channel();

        // Spawn the sparse trie task using any stored trie and parallel trie configuration.
        self.spawn_sparse_trie_task(sparse_trie_rx, proof_handle, state_root_tx);

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
    pub(super) fn spawn_cache_exclusive<P, I: ExecutableTxIterator<Evm>>(
        &self,
        env: ExecutionEnv<Evm>,
        transactions: I,
        provider_builder: StateProviderBuilder<N, P>,
        bal: Option<Arc<BlockAccessList>>,
    ) -> IteratorPayloadHandle<Evm, I, N>
    where
        P: BlockReader + StateProviderFactory + StateReader + Clone + 'static,
    {
        let (prewarm_rx, execution_rx, size_hint) = self.spawn_tx_iterator(transactions);
        let prewarm_handle =
            self.spawn_caching_with(env, prewarm_rx, size_hint, provider_builder, None, bal);
        PayloadHandle {
            to_multi_proof: None,
            prewarm_handle,
            state_root: None,
            transactions: execution_rx,
            _span: Span::current(),
        }
    }

    /// Spawns a task advancing transaction env iterator and streaming updates through a channel.
    #[expect(clippy::type_complexity)]
    fn spawn_tx_iterator<I: ExecutableTxIterator<Evm>>(
        &self,
        transactions: I,
    ) -> (
        mpsc::Receiver<WithTxEnv<TxEnvFor<Evm>, I::Tx>>,
        mpsc::Receiver<Result<WithTxEnv<TxEnvFor<Evm>, I::Tx>, I::Error>>,
        usize,
    ) {
        let (transactions, convert) = transactions.into();
        let transactions = transactions.into_par_iter();
        let transaction_count_hint = transactions.len();

        let (ooo_tx, ooo_rx) = mpsc::channel();
        let (prewarm_tx, prewarm_rx) = mpsc::channel();
        let (execute_tx, execute_rx) = mpsc::channel();

        // Spawn a task that `convert`s all transactions in parallel and sends them out-of-order.
        self.executor.spawn_blocking(move || {
            transactions.enumerate().for_each_with(ooo_tx, |ooo_tx, (idx, tx)| {
                let tx = convert(tx);
                let tx = tx.map(|tx| WithTxEnv { tx_env: tx.to_tx_env(), tx: Arc::new(tx) });
                // Only send Ok(_) variants to prewarming task.
                if let Ok(tx) = &tx {
                    let _ = prewarm_tx.send(tx.clone());
                }
                let _ = ooo_tx.send((idx, tx));
            });
        });

        // Spawn a task that processes out-of-order transactions from the task above and sends them
        // to the execution task in order.
        self.executor.spawn_blocking(move || {
            let mut next_for_execution = 0;
            let mut queue = BTreeMap::new();
            while let Ok((idx, tx)) = ooo_rx.recv() {
                if next_for_execution == idx {
                    let _ = execute_tx.send(tx);
                    next_for_execution += 1;

                    while let Some(entry) = queue.first_entry() &&
                        *entry.key() == next_for_execution
                    {
                        let _ = execute_tx.send(entry.remove());
                        next_for_execution += 1;
                    }
                } else {
                    queue.insert(idx, tx);
                }
            }
        });

        (prewarm_rx, execute_rx, transaction_count_hint)
    }

    /// Spawn prewarming optionally wired to the multiproof task for target updates.
    fn spawn_caching_with<P>(
        &self,
        env: ExecutionEnv<Evm>,
        mut transactions: mpsc::Receiver<impl ExecutableTxFor<Evm> + Clone + Send + 'static>,
        transaction_count_hint: usize,
        provider_builder: StateProviderBuilder<N, P>,
        to_multi_proof: Option<CrossbeamSender<MultiProofMessage>>,
        bal: Option<Arc<BlockAccessList>>,
    ) -> CacheTaskHandle<N::Receipt>
    where
        P: BlockReader + StateProviderFactory + StateReader + Clone + 'static,
    {
        if self.disable_transaction_prewarming {
            // if no transactions should be executed we clear them but still spawn the task for
            // caching updates
            transactions = mpsc::channel().1;
        }

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
            transaction_count_hint,
            self.prewarm_max_concurrency,
        );

        // spawn pre-warm task
        {
            let to_prewarm_task = to_prewarm_task.clone();
            self.executor.spawn_blocking(move || {
                let mode = if let Some(bal) = bal {
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
            let cache = crate::tree::cached_state::ExecutionCache::new(self.cross_block_cache_size);
            SavedCache::new(
                parent_hash,
                cache,
                CachedStateMetrics::zeroed(),
                FixedCacheMetrics::zeroed(),
            )
        }
    }

    /// Spawns the [`SparseTrieTask`] for this payload processor.
    #[instrument(level = "debug", target = "engine::tree::payload_processor", skip_all)]
    fn spawn_sparse_trie_task<BPF>(
        &self,
        sparse_trie_rx: mpsc::Receiver<SparseTrieUpdate>,
        proof_worker_handle: BPF,
        state_root_tx: mpsc::Sender<Result<StateRootComputeOutcome, ParallelStateRootError>>,
    ) where
        BPF: TrieNodeProviderFactory + Clone + Send + Sync + 'static,
        BPF::AccountNodeProvider: TrieNodeProvider + Send + Sync,
        BPF::StorageNodeProvider: TrieNodeProvider + Send + Sync,
    {
        // Reuse a stored SparseStateTrie, or create a new one using the desired configuration if
        // there's none to reuse.
        let cleared_sparse_trie = Arc::clone(&self.sparse_state_trie);
        let sparse_state_trie = cleared_sparse_trie.lock().take().unwrap_or_else(|| {
            let default_trie = SparseTrie::blind_from(if self.disable_parallel_sparse_trie {
                ConfiguredSparseTrie::Serial(Default::default())
            } else {
                ConfiguredSparseTrie::Parallel(Box::new(
                    ParallelSparseTrie::default()
                        .with_parallelism_thresholds(PARALLEL_SPARSE_TRIE_PARALLELISM_THRESHOLDS),
                ))
            });
            ClearedSparseStateTrie::from_state_trie(
                SparseStateTrie::new()
                    .with_accounts_trie(default_trie.clone())
                    .with_default_storage_trie(default_trie)
                    .with_updates(true),
            )
        });

        let task =
            SparseTrieTask::<_, ConfiguredSparseTrie, ConfiguredSparseTrie>::new_with_cleared_trie(
                sparse_trie_rx,
                proof_worker_handle,
                self.trie_metrics.clone(),
                sparse_state_trie,
            );

        let span = Span::current();
        self.executor.spawn_blocking(move || {
            let _enter = span.entered();

            let (result, trie) = task.run();
            // Send state root computation result
            let _ = state_root_tx.send(result);

            // Clear the SparseStateTrie, shrink, and replace it back into the mutex _after_ sending
            // results to the next step, so that time spent clearing doesn't block the step after
            // this one.
            let _enter = debug_span!(target: "engine::tree::payload_processor", "clear").entered();
            let mut cleared_trie = ClearedSparseStateTrie::from_state_trie(trie);

            // Shrink the sparse trie so that we don't have ever increasing memory.
            cleared_trie.shrink_to(
                SPARSE_TRIE_MAX_NODES_SHRINK_CAPACITY,
                SPARSE_TRIE_MAX_VALUES_SHRINK_CAPACITY,
            );

            cleared_sparse_trie.lock().replace(cleared_trie);
        });
    }

    /// Updates the execution cache with the post-execution state from an inserted block.
    ///
    /// This is used when blocks are inserted directly (e.g., locally built blocks by sequencers)
    /// to ensure the cache remains warm for subsequent block execution.
    ///
    /// The cache enables subsequent blocks to reuse account, storage, and bytecode data without
    /// hitting the database, maintaining performance consistency.
    pub(crate) fn on_inserted_executed_block(
        &self,
        block_with_parent: BlockWithParent,
        bundle_state: &BundleState,
    ) {
        self.execution_cache.update_with_guard(|cached| {
            if cached.as_ref().is_some_and(|c| c.executed_block_hash() != block_with_parent.parent) {
                debug!(
                    target: "engine::caching",
                    parent_hash = %block_with_parent.parent,
                    "Cannot find cache for parent hash, skip updating cache with new state for inserted executed block",
                );
                return;
            }

            // Take existing cache (if any) or create fresh caches
            let (caches, cache_metrics, fixed_cache_metrics) = match cached.take() {
                Some(existing) => existing.split(),
                None => (
                    crate::tree::cached_state::ExecutionCache::new(self.cross_block_cache_size),
                    CachedStateMetrics::zeroed(),
                    FixedCacheMetrics::zeroed(),
                ),
            };

            // Insert the block's bundle state into cache
            let new_cache = SavedCache::new(
                block_with_parent.block.hash,
                caches,
                cache_metrics,
                fixed_cache_metrics,
            );
            if new_cache.cache().insert_state(bundle_state).is_err() {
                *cached = None;
                debug!(target: "engine::caching", "cleared execution cache on update error");
                return;
            }
            new_cache.update_metrics();

            // Replace with the updated cache
            *cached = Some(new_cache);
            debug!(target: "engine::caching", ?block_with_parent, "Updated execution cache for inserted block");
        });
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
    pub(super) fn caches(&self) -> Option<StateExecutionCache> {
        self.prewarm_handle.saved_cache.as_ref().map(|cache| cache.cache().clone())
    }

    /// Returns a clone of the cache metrics used by prewarming
    pub(super) fn cache_metrics(&self) -> Option<CachedStateMetrics> {
        self.prewarm_handle.saved_cache.as_ref().map(|cache| cache.metrics().clone())
    }

    /// Terminates the pre-warming transaction processing.
    ///
    /// Note: This does not terminate the task yet.
    pub(super) fn stop_prewarming_execution(&self) {
        self.prewarm_handle.stop_prewarming_execution()
    }

    /// Terminates the entire caching task.
    ///
    /// If the [`ExecutionOutcome`] is provided it will update the shared cache using its
    /// bundle state. Using `Arc<ExecutionOutcome>` allows sharing with the main execution
    /// path without cloning the expensive `BundleState`.
    pub(super) fn terminate_caching(
        &mut self,
        execution_outcome: Option<Arc<ExecutionOutcome<R>>>,
    ) {
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
pub(crate) struct CacheTaskHandle<R> {
    /// The shared cache the task operates with.
    saved_cache: Option<SavedCache>,
    /// Channel to the spawned prewarm task if any
    to_prewarm_task: Option<std::sync::mpsc::Sender<PrewarmTaskEvent<R>>>,
}

impl<R: Send + Sync + 'static> CacheTaskHandle<R> {
    /// Terminates the pre-warming transaction processing.
    ///
    /// Note: This does not terminate the task yet.
    pub(super) fn stop_prewarming_execution(&self) {
        self.to_prewarm_task
            .as_ref()
            .map(|tx| tx.send(PrewarmTaskEvent::TerminateTransactionExecution).ok());
    }

    /// Terminates the entire pre-warming task.
    ///
    /// If the [`ExecutionOutcome`] is provided it will update the shared cache using its
    /// bundle state. Using `Arc<ExecutionOutcome>` avoids cloning the expensive `BundleState`.
    pub(super) fn terminate_caching(
        &mut self,
        execution_outcome: Option<Arc<ExecutionOutcome<R>>>,
    ) {
        if let Some(tx) = self.to_prewarm_task.take() {
            let event = PrewarmTaskEvent::Terminate { execution_outcome };
            let _ = tx.send(event);
        }
    }
}

impl<R> Drop for CacheTaskHandle<R> {
    fn drop(&mut self) {
        // Ensure we always terminate on drop - send None without needing Send + Sync bounds
        if let Some(tx) = self.to_prewarm_task.take() {
            let _ = tx.send(PrewarmTaskEvent::Terminate { execution_outcome: None });
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
/// (such as prewarming tasks) must be terminated before calling `update_with_guard`, otherwise
/// the cache may be corrupted or cleared.
///
/// ## Cache vs Prewarming Distinction
///
/// **`ExecutionCache`**:
/// - Stores parent block's execution state after completion
/// - Used to fetch parent data for next block's execution
/// - Must be exclusively accessed during save operations
///
/// **`PrewarmCacheTask`**:
/// - Speculatively loads accounts/storage that might be used in transaction execution
/// - Prepares data for state root proof computation
/// - Runs concurrently but must not interfere with cache saves
#[derive(Clone, Debug, Default)]
struct ExecutionCache {
    /// Guarded cloneable cache identified by a block hash.
    inner: Arc<RwLock<Option<SavedCache>>>,
}

impl ExecutionCache {
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

            if hash_matches && available {
                return Some(c.clone());
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
    pub(crate) fn update_with_guard<F>(&self, update_fn: F)
    where
        F: FnOnce(&mut Option<SavedCache>),
    {
        let mut guard = self.inner.write();
        update_fn(&mut guard);
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
}

impl<Evm: ConfigureEvm> Default for ExecutionEnv<Evm>
where
    EvmEnvFor<Evm>: Default,
{
    fn default() -> Self {
        Self {
            evm_env: Default::default(),
            hash: Default::default(),
            parent_hash: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ExecutionCache;
    use crate::tree::{
        cached_state::{CachedStateMetrics, FixedCacheMetrics, SavedCache},
        payload_processor::{
            evm_state_to_hashed_post_state, executor::WorkloadExecutor, PayloadProcessor,
        },
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
    use revm_primitives::{Address, HashMap, B256, KECCAK_EMPTY, U256};
    use revm_state::{AccountInfo, AccountStatus, EvmState, EvmStorageSlot};
    use std::sync::Arc;

    fn make_saved_cache(hash: B256) -> SavedCache {
        let execution_cache = crate::tree::cached_state::ExecutionCache::new(1_000);
        SavedCache::new(
            hash,
            execution_cache,
            CachedStateMetrics::zeroed(),
            FixedCacheMetrics::zeroed(),
        )
    }

    #[test]
    fn execution_cache_allows_single_checkout() {
        let execution_cache = ExecutionCache::default();
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
        let execution_cache = ExecutionCache::default();
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
    fn execution_cache_mismatch_parent_returns_none() {
        let execution_cache = ExecutionCache::default();
        let hash = B256::from([3u8; 32]);

        execution_cache.update_with_guard(|slot| *slot = Some(make_saved_cache(hash)));

        let miss = execution_cache.get_cache_for(B256::from([4u8; 32]));
        assert!(miss.is_none(), "checkout should fail for different parent hash");
    }

    #[test]
    fn execution_cache_update_after_release_succeeds() {
        let execution_cache = ExecutionCache::default();
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
            WorkloadExecutor::default(),
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
            WorkloadExecutor::default(),
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
                    },
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
            WorkloadExecutor::default(),
            EthEvmConfig::new(factory.chain_spec()),
            &TreeConfig::default(),
            PrecompileCacheMap::default(),
        );

        let provider_factory = BlockchainProvider::new(factory).unwrap();

        let mut handle = payload_processor.spawn(
            Default::default(),
            (
                Vec::<Result<Recovered<TransactionSigned>, core::convert::Infallible>>::new(),
                std::convert::identity,
            ),
            StateProviderBuilder::new(provider_factory.clone(), genesis_hash, None),
            OverlayStateProviderFactory::new(provider_factory),
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
