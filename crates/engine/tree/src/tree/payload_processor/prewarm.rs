//! Caching and prewarming related functionality.
//!
//! Prewarming executes transactions in parallel before the actual block execution
//! to populate the execution cache with state that will likely be accessed during
//! block processing.
//!
//! ## How Prewarming Works
//!
//! 1. Incoming transactions are split into two streams: one for prewarming (executed in parallel)
//!    and one for actual execution (executed sequentially)
//! 2. Prewarming tasks execute transactions in parallel using shared caches
//! 3. When actual block execution happens, it benefits from the warmed cache

use super::{bal_prewarm_pool::BalPrewarmPool, StateRootHintStream, StateRootUpdateStream};
use crate::tree::{
    precompile_cache::{CachedPrecompile, PrecompileCacheMap},
    CachedStateCacheMetrics, CachedStateMetrics, CachedStateProvider, ExecutionEnv,
    PayloadExecutionCache, SavedCache,
};
use alloy_consensus::transaction::TxHashRef;
use alloy_eip7928::bal::DecodedBal;
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::keccak256;
use metrics::{Counter, Gauge, Histogram};
use rayon::prelude::*;
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm, Evm, EvmFor, RecoveredTx, SpecFor};
use reth_metrics::Metrics;
use reth_primitives_traits::{FastInstant as Instant, NodePrimitives};
use reth_provider::{
    AccountReader, BlockExecutionOutput, BlockNumReader, ChangeSetReader, DatabaseProviderFactory,
    DatabaseProviderROFactory, HistoryReader, PruneCheckpointReader, StageCheckpointReader,
    StateProviderBox, StorageChangeSetReader, StorageSettingsCache,
};
use reth_revm::database::StateProviderDatabase;
use reth_storage_overlay::OverlayStateProviderFactory;
use reth_tasks::{pool::WorkerPool, Runtime};
use reth_trie_common::MultiProofTargetsV2;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::{self, channel, Receiver, Sender},
    Arc,
};
use tokio::sync::oneshot;
use tracing::{debug, debug_span, instrument, trace, trace_span, warn, Span};

/// Determines the prewarming mode: transaction-based, BAL-based, or skipped.
///
/// Each variant carries the state-root capability its producers use, so the capability dies
/// with the workers instead of outliving them.
#[derive(Debug)]
pub enum PrewarmMode<Tx> {
    /// Prewarm by executing transactions from a stream, each paired with its block index.
    Transactions {
        /// Stream of transactions pending prewarm execution.
        pending: Receiver<(usize, Tx)>,
        /// Best-effort access hints emitted by the prewarm workers.
        hints: Option<StateRootHintStream>,
    },
    /// Prewarm by prefetching slots from a Block Access List.
    BlockAccessList {
        /// The decoded block access list.
        bal: Arc<DecodedBal>,
        /// Authoritative pre-hashed updates derived from the BAL.
        updates: Option<StateRootUpdateStream>,
    },
    /// Transaction prewarming is skipped (e.g. small blocks where the overhead exceeds the
    /// benefit). No workers are spawned.
    Skipped,
}

/// A task that is responsible for caching and prewarming the cache by executing transactions
/// individually in parallel.
///
/// Note: This task runs until cancelled externally.
#[derive(Debug)]
pub struct PrewarmCacheTask<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    /// The executor used to spawn execution tasks.
    executor: Runtime,
    /// Shared execution cache.
    execution_cache: PayloadExecutionCache,
    /// Context provided to execution tasks
    ctx: PrewarmContext<N, P, Evm>,
    /// Receiver for events produced by tx execution
    actions_rx: Receiver<PrewarmTaskEvent<N::Receipt>>,
    /// Parent span for tracing
    parent_span: Span,
}

impl<N, P, Evm> PrewarmCacheTask<N, P, Evm>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory + Clone + 'static,
    P::Provider: BlockNumReader
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + HistoryReader
        + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    /// Initializes the task with the given transactions pending execution
    pub fn new(
        executor: Runtime,
        execution_cache: PayloadExecutionCache,
        ctx: PrewarmContext<N, P, Evm>,
    ) -> (Self, Sender<PrewarmTaskEvent<N::Receipt>>) {
        let (actions_tx, actions_rx) = channel();

        trace!(
            target: "engine::tree::payload_processor::prewarm",
            prewarming_threads = executor.prewarming_pool().current_num_threads(),
            transaction_count = ctx.env.transaction_count,
            "Initialized prewarm task"
        );

        (
            Self { executor, execution_cache, ctx, actions_rx, parent_span: Span::current() },
            actions_tx,
        )
    }

    /// Streams pending transactions and executes them in parallel on the prewarming pool.
    ///
    /// Kicks off EVM init on every pool thread, then uses `in_place_scope` to dispatch
    /// transactions as they arrive and wait for all spawned tasks to complete before
    /// clearing per-thread state. Workers that start via work-stealing lazily initialise
    /// their EVM state on first access via [`get_or_init`](reth_tasks::pool::Worker::get_or_init).
    fn spawn_txs_prewarm<Tx>(
        &self,
        pending: mpsc::Receiver<(usize, Tx)>,
        actions_tx: Sender<PrewarmTaskEvent<N::Receipt>>,
        state_root_hint_stream: Option<StateRootHintStream>,
    ) where
        Tx: ExecutableTxFor<Evm> + Send + 'static,
    {
        let executor = self.executor.clone();
        let ctx = self.ctx.clone();
        let span = Span::current();

        self.executor.spawn_blocking_named("prewarm-txs", move || {
            let _enter = debug_span!(
                target: "engine::tree::payload_processor::prewarm",
                parent: &span,
                "prewarm_txs"
            )
            .entered();

            let ctx = &ctx;
            let pool = executor.prewarming_pool();

            let mut tx_count = 0usize;
            let state_root_hint_stream = state_root_hint_stream.as_ref();
            pool.in_place_scope(|s| {
                s.spawn(|_| {
                    pool.init::<PrewarmEvmState<Evm>>(|_| ctx.evm_for_ctx());
                });

                while let Ok((index, tx)) = pending.recv() {
                    if ctx.should_stop() {
                        trace!(
                            target: "engine::tree::payload_processor::prewarm",
                            "Termination requested, stopping transaction distribution"
                        );
                        break;
                    }

                    // skip transactions already executed by the main loop
                    if index < ctx.executed_tx_index.load(Ordering::Relaxed) {
                        continue;
                    }

                    tx_count += 1;
                    let parent_span = Span::current();
                    s.spawn(move |_| {
                        let _enter = trace_span!(
                            target: "engine::tree::payload_processor::prewarm",
                            parent: parent_span,
                            "prewarm_tx",
                            i = index,
                        )
                        .entered();
                        Self::transact_worker(ctx, index, tx, state_root_hint_stream);
                    });
                }

                // Send withdrawal prefetch targets after all transactions dispatched
                if let Some(state_root_hint_stream) = state_root_hint_stream &&
                    let Some(withdrawals) = &ctx.env.withdrawals &&
                    !withdrawals.is_empty()
                {
                    let targets = multiproof_targets_from_withdrawals(withdrawals);
                    state_root_hint_stream.on_access_hint(targets.into());
                }
            });

            // All tasks are done — clear per-thread EVM state for the next block.
            pool.clear();

            let _ = actions_tx
                .send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: tx_count });
        });
    }

    /// Executes a single prewarm transaction on the current pool thread's EVM.
    ///
    /// Lazily initialises per-thread [`PrewarmEvmState`] via
    /// [`get_or_init`](reth_tasks::pool::Worker::get_or_init) on first access.
    fn transact_worker<Tx>(
        ctx: &PrewarmContext<N, P, Evm>,
        index: usize,
        tx: Tx,
        state_root_hint_stream: Option<&StateRootHintStream>,
    ) where
        Tx: ExecutableTxFor<Evm>,
    {
        WorkerPool::with_worker_mut(|worker| {
            let Some(evm) =
                worker.get_or_init::<PrewarmEvmState<Evm>>(|| ctx.evm_for_ctx()).as_mut()
            else {
                return;
            };

            if ctx.should_stop() {
                return;
            }

            // skip if main execution has already processed this transaction
            if index < ctx.executed_tx_index.load(Ordering::Relaxed) {
                return;
            }

            let start = Instant::now();

            let (tx_env, tx) = tx.into_parts();
            let res = match evm.transact(tx_env) {
                Ok(res) => res,
                Err(err) => {
                    trace!(
                        target: "engine::tree::payload_processor::prewarm",
                        %err,
                        tx_hash=%tx.tx().tx_hash(),
                        sender=%tx.signer(),
                        "Error when executing prewarm transaction",
                    );
                    ctx.metrics.transaction_errors.increment(1);
                    return;
                }
            };
            ctx.metrics.execution_duration.record(start.elapsed());

            if ctx.should_stop() {
                return;
            }

            if index > 0 {
                let (targets, storage_targets) = MultiProofTargetsV2::from_state(res.state);
                ctx.metrics.prefetch_storage_targets.record(storage_targets as f64);
                if let Some(state_root_hint_stream) = state_root_hint_stream {
                    state_root_hint_stream.on_access_hint(targets.into());
                }
            }

            ctx.metrics.total_runtime.record(start.elapsed());
        });
    }

    /// Saves the warmed cache in `self.execution_cache` after prewarming completes.
    ///
    /// This method calls [`PayloadExecutionCache::update_with_guard`], which requires exclusive
    /// access. It should only be called after ensuring that:
    /// 1. All prewarming tasks have completed execution
    /// 2. No other concurrent operations are accessing the cache
    ///
    /// This moves the task's `ExecutionCache` into `self.execution_cache` when the block is valid,
    /// without retaining an extra Arc reference that would prevent reuse after unlocking.
    /// This method is called from `run()` only after all execution tasks are complete.
    ///
    /// State insertion and block validation run under the mutex because the cache being updated
    /// may also be stored in `self.execution_cache`. Removed `SavedCache` values are dropped after
    /// unlocking, before the next prewarm task. Their contents are freed only when the last
    /// `ExecutionCache` clone is dropped, which may happen later on another thread.
    #[instrument(level = "debug", target = "engine::tree::payload_processor::prewarm", skip_all)]
    fn save_cache(
        self,
        execution_outcome: Arc<BlockExecutionOutput<N::Receipt>>,
        valid_block_rx: mpsc::Receiver<()>,
    ) {
        let start = Instant::now();

        let Self {
            execution_cache,
            ctx: PrewarmContext { env, metrics, cache_state_metrics, saved_cache, .. },
            ..
        } = self;
        let hash = env.hash;

        if let Some(saved_cache) = saved_cache {
            debug!(target: "engine::caching", parent_hash=?hash, "Updating execution cache");
            let (previous, rejected) = execution_cache.update_with_guard(|cached| {
                let new_cache = SavedCache::new(hash, saved_cache.into_cache());

                // Update under the mutex so no checkout can observe partially updated state.
                if new_cache.cache().insert_state(&execution_outcome.state).is_err() {
                    debug!(target: "engine::caching", "cleared execution cache on update error");
                    return (cached.take(), Some(new_cache));
                }

                new_cache.update_metrics(cache_state_metrics.as_ref());

                // `cached` and `new_cache` can point to the same cache.
                // insert_state has already applied this block's changes to it, so keep the mutex
                // locked until validation succeeds or we clear `cached` on failure.
                if valid_block_rx.recv().is_err() {
                    debug!(target: "engine::caching", "cleared execution cache on invalid block");
                    return (cached.take(), Some(new_cache));
                }

                let reused =
                    cached.as_ref().is_some_and(|previous| previous.shares_cache_with(&new_cache));
                let previous = cached.replace(new_cache);
                if reused {
                    // `cached` and `previous` hold the same cache. Drop the extra Arc reference
                    // before unlocking so get_cache_for can reuse it.
                    drop(previous);
                    (None, None)
                } else {
                    // Drop the old cache after unlocking; freeing it may be expensive.
                    (previous, None)
                }
            });

            // Drop these SavedCache values on this worker, without a background drop queue.
            // This frees their contents only if no other ExecutionCache clones remain.
            // Otherwise, the thread dropping the last ExecutionCache clone frees them later.
            // Another payload may access or allocate a cache while these drops run.
            drop((previous, rejected));

            let elapsed = start.elapsed();
            debug!(target: "engine::caching", parent_hash=?hash, elapsed=?elapsed, "Updated execution cache");

            metrics.cache_saving_duration.set(elapsed.as_secs_f64());
        }
    }

    /// Runs BAL-based prewarming and state-root streaming inline.
    ///
    /// Spawns two halves concurrently on separate pools, then waits for both to complete:
    /// 1. Hashed state streaming on the BAL streaming pool so storage updates can reach the
    ///    state-root job before account reads finish.
    /// 2. Storage prefetch on the prewarming pool to populate the execution cache, unless BAL batch
    ///    I/O is disabled.
    #[instrument(level = "debug", target = "engine::tree::payload_processor::prewarm", skip_all)]
    fn run_bal_prewarm(
        &self,
        decoded_bal: Arc<DecodedBal>,
        actions_tx: Sender<PrewarmTaskEvent<N::Receipt>>,
        hashed_update_stream: Option<StateRootUpdateStream>,
    ) {
        let bal = decoded_bal.as_bal();
        if bal.is_empty() {
            if let Some(hashed_update_stream) = hashed_update_stream {
                hashed_update_stream.finish();
            }
            let _ =
                actions_tx.send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: 0 });
            return;
        }

        trace!(
            target: "engine::tree::payload_processor::prewarm",
            accounts = bal.len(),
            "Starting BAL prewarm"
        );

        let ctx = self.ctx.clone();
        let executor = self.executor.clone();
        let parent_span = Span::current();
        let stream_parent_span = parent_span;
        let prefetch_bal = Arc::clone(&decoded_bal);
        let stream_bal = Arc::clone(&decoded_bal);
        let (stream_tx, stream_rx) = oneshot::channel();

        if let Some(hashed_update_stream) = hashed_update_stream {
            let ctx = ctx.clone();
            executor.bal_streaming_pool().spawn(move || {
                let branch_span = debug_span!(
                    target: "engine::tree::payload_processor::prewarm",
                    parent: &stream_parent_span,
                    "bal_hashed_state_stream",
                    bal_accounts = stream_bal.as_bal().len(),
                );
                let parent_span = branch_span.clone();
                let _span = branch_span.entered();

                stream_bal.as_bal().par_iter().for_each(|account_changes| {
                    WorkerPool::with_worker_mut(|worker| {
                        let provider =
                            worker.get_or_init::<Option<Box<dyn AccountReader>>>(|| None);
                        ctx.send_bal_hashed_state(
                            &parent_span,
                            provider,
                            account_changes,
                            &hashed_update_stream,
                        );
                    });
                });

                hashed_update_stream.finish();
                let _ = stream_tx.send(());
            });
        } else {
            let _ = stream_tx.send(());
        }

        if let Some(saved_cache) = ctx.saved_cache &&
            !ctx.disable_bal_batch_io &&
            let Some(pool) = ctx.bal_prewarm_pool.as_ref()
        {
            // If
            //
            // - BAL path is enabled (and so bal_prewarm_pool is present),
            // - dispatch_bal_batch_io is false
            // - execution cache is not disabled
            //
            // we launch prewarming sequence of the BAL read set here. The BAL read-set consists
            // of the accounts, their code if present, and declared storages (both storage_reads
            // and storage_changes).
            //
            // This runs side-by-side with the parallel transaction execution reducing the time it
            // spends blocking on the data.
            let caches = saved_cache.cache().clone();
            let state_provider_factory = ctx.provider.clone();
            let build = Arc::new(move || {
                state_provider_factory
                    .database_provider_ro()
                    .map(|provider| Box::new(provider) as _)
            });

            pool.begin_block(build, caches, ctx.env.txpool_snapshot.clone());
            let dispatch_start = Instant::now();
            for account in prefetch_bal.as_bal() {
                pool.warm_account(account.address, account.storage_slots().map(Into::into));
            }
            ctx.metrics.bal_slot_iteration_duration.record(dispatch_start.elapsed());
            pool.end_block();
        }

        stream_rx
            .blocking_recv()
            .expect("BAL hashed-state streaming task dropped without signaling completion");

        // Drop the per-thread providers
        executor.bal_streaming_pool().clear();

        let _ = actions_tx.send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: 0 });
    }

    /// Executes the task.
    ///
    /// This will execute the transactions until all transactions have been processed or the task
    /// was cancelled.
    #[instrument(
        parent = &self.parent_span,
        level = "debug",
        target = "engine::tree::payload_processor::prewarm",
        name = "prewarm and caching",
        skip_all
    )]
    pub fn run<Tx>(self, mode: PrewarmMode<Tx>, actions_tx: Sender<PrewarmTaskEvent<N::Receipt>>)
    where
        Tx: ExecutableTxFor<Evm> + Send + 'static,
    {
        // Spawn execution tasks based on mode. The state-root capabilities arrive inside the
        // mode and move into the spawned producers, so they die with the producers instead of
        // living for the full lifetime of this task.
        match mode {
            PrewarmMode::Transactions { pending, hints } => {
                self.spawn_txs_prewarm(pending, actions_tx, hints);
            }
            PrewarmMode::BlockAccessList { bal, updates } => {
                self.run_bal_prewarm(bal, actions_tx, updates);
            }
            PrewarmMode::Skipped => {
                let _ = actions_tx
                    .send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: 0 });
            }
        }

        let mut final_execution_outcome = None;
        let mut finished_execution = false;
        while let Ok(event) = self.actions_rx.recv() {
            match event {
                PrewarmTaskEvent::TerminateTransactionExecution => {
                    // stop tx processing
                    debug!(target: "engine::tree::prewarm", "Terminating prewarm execution");
                    self.ctx.stop();
                }
                PrewarmTaskEvent::Terminate { execution_outcome, valid_block_rx } => {
                    trace!(target: "engine::tree::payload_processor::prewarm", "Received termination signal");
                    // `Terminate` can arrive without `TerminateTransactionExecution` when the
                    // handle is dropped on an execution error, so stop workers before waiting.
                    self.ctx.stop();
                    final_execution_outcome =
                        Some(execution_outcome.map(|outcome| (outcome, valid_block_rx)));

                    if finished_execution {
                        // all tasks are done, we can exit, which will save caches and exit
                        break
                    }
                }
                PrewarmTaskEvent::FinishedTxExecution { executed_transactions } => {
                    trace!(target: "engine::tree::payload_processor::prewarm", "Finished prewarm execution signal");
                    self.ctx.metrics.transactions.set(executed_transactions as f64);
                    self.ctx.metrics.transactions_histogram.record(executed_transactions as f64);

                    finished_execution = true;

                    if final_execution_outcome.is_some() {
                        // all tasks are done, we can exit, which will save caches and exit
                        break
                    }
                }
            }
        }

        debug!(target: "engine::tree::payload_processor::prewarm", "Completed prewarm execution");

        // save caches and finish using the shared ExecutionOutcome
        if let Some(Some((execution_outcome, valid_block_rx))) = final_execution_outcome {
            self.save_cache(execution_outcome, valid_block_rx);
        }
    }
}

/// Context required by tx execution tasks.
#[derive(Debug, Clone)]
pub struct PrewarmContext<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    /// The execution environment.
    pub env: ExecutionEnv<Evm>,
    /// The EVM configuration.
    pub evm_config: Evm,
    /// The saved cache.
    pub saved_cache: Option<SavedCache>,
    /// Provider to obtain the state
    pub provider: OverlayStateProviderFactory<P, N>,
    /// Dedicated blocking pool for warming the BAL read-set. `Some` only on the BAL parallel
    /// execution path; the pool is owned by the [`PayloadProcessor`](super::PayloadProcessor).
    pub(crate) bal_prewarm_pool: Option<Arc<BalPrewarmPool>>,
    /// The metrics for the prewarm task.
    pub metrics: PrewarmMetrics,
    /// Metrics for the execution cache.
    /// Metrics for the execution cache. `None` disables metrics recording.
    pub cache_metrics: Option<CachedStateMetrics>,
    /// Metrics for shared execution cache state. `None` disables metrics recording.
    pub cache_state_metrics: Option<CachedStateCacheMetrics>,
    /// An atomic bool that tells prewarm tasks to not start any more execution.
    pub terminate_execution: Arc<AtomicBool>,
    /// Shared counter tracking the next transaction index to be executed by the main execution
    /// loop. Prewarm workers skip transactions with `index < counter` since those have already
    /// been executed.
    pub executed_tx_index: Arc<AtomicUsize>,
    /// Whether the precompile cache is disabled.
    pub precompile_cache_disabled: bool,
    /// The precompile cache map.
    pub precompile_cache_map: PrecompileCacheMap<SpecFor<Evm>>,
    /// Whether to disable BAL-driven parallel state root computation.
    /// Only valid when BAL parallel execution is also disabled.
    pub disable_bal_parallel_state_root: bool,
    /// Whether BAL state prefetching during prewarm is disabled.
    pub disable_bal_batch_io: bool,
}

/// Per-thread EVM state initialised by [`PrewarmContext::evm_for_ctx`] and stored in
/// [`WorkerPool`] workers via [`Worker::get_or_init`](reth_tasks::pool::Worker::get_or_init).
type PrewarmEvmState<Evm> =
    Option<EvmFor<Evm, StateProviderDatabase<reth_provider::StateProviderBox>>>;

impl<N, P, Evm> PrewarmContext<N, P, Evm>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory,
    P::Provider: BlockNumReader
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + HistoryReader
        + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    /// Creates a per-thread EVM for prewarming.
    #[instrument(level = "debug", target = "engine::tree::payload_processor::prewarm", skip_all)]
    fn evm_for_ctx(&self) -> PrewarmEvmState<Evm> {
        let mut state_provider: StateProviderBox = match self.provider.database_provider_ro() {
            Ok(provider) => Box::new(provider),
            Err(err) => {
                trace!(
                    target: "engine::tree::payload_processor::prewarm",
                    %err,
                    "Failed to build state provider in prewarm thread"
                );
                return None
            }
        };

        // Use the caches to create a new provider with caching
        if let Some(saved_cache) = &self.saved_cache {
            let caches = saved_cache.cache().clone();
            state_provider = Box::new(
                CachedStateProvider::new_prewarm(state_provider, caches)
                    .with_txpool_snapshot(self.env.txpool_snapshot.clone()),
            );
        }

        let state_provider = StateProviderDatabase::new(state_provider);

        let mut evm_env = self.env.evm_env.clone();

        // we must disable the nonce check so that we can execute the transaction even if the nonce
        // doesn't match what's on chain.
        evm_env.cfg_env.disable_nonce_check = true;

        // disable the balance check so that transactions from senders who were funded by earlier
        // transactions in the block can still be prewarmed
        evm_env.cfg_env.disable_balance_check = true;

        // create a new executor and disable nonce checks in the env
        let spec_id = *evm_env.spec_id();
        let mut evm = self.evm_config.evm_with_env(state_provider, evm_env);

        if !self.precompile_cache_disabled {
            // Only cache pure precompiles to avoid issues with stateful precompiles
            evm.precompiles_mut().map_cacheable_precompiles(|address, precompile| {
                CachedPrecompile::wrap(
                    precompile,
                    self.precompile_cache_map.cache_for_address(*address),
                    spec_id,
                    None, // No metrics for prewarm
                )
            });
        }

        Some(evm)
    }

    /// Returns `true` if prewarming should stop.
    #[inline]
    pub fn should_stop(&self) -> bool {
        self.terminate_execution.load(Ordering::Relaxed)
    }

    /// Signals all prewarm tasks to stop execution.
    #[inline]
    pub fn stop(&self) {
        self.terminate_execution.store(true, Ordering::Relaxed);
    }

    /// Hashes and streams a single BAL account's state to the state-root job's hashed-update
    /// stream.
    ///
    /// For each changed account, storage slots are hashed and sent immediately, then the account
    /// is sent as a separate update. The parent account is read only when the BAL did not provide
    /// all account leaf fields needed for state-root computation.
    ///
    /// The `provider` is lazily initialized on first call and reused across accounts on the same
    /// thread.
    fn send_bal_hashed_state(
        &self,
        parent_span: &Span,
        provider: &mut Option<Box<dyn AccountReader>>,
        account_changes: &alloy_eip7928::AccountChanges,
        hashed_update_stream: &StateRootUpdateStream,
    ) {
        if self.disable_bal_parallel_state_root {
            return;
        }
        let address = account_changes.address;
        let mut hashed_address = None;
        let account_info = account_changes.account_info();

        if !account_info.changes_state_root(account_changes) {
            return;
        }

        // If there are any storage changes we can assume that the resulting account info will be
        // non-empty, so the account will exist, and therefore we can pre-emptively send out storage
        // changes to start processing them before potentially hitting the db in the next step.
        if account_changes.has_storage_changes() {
            let hashed_address = *hashed_address.get_or_insert_with(|| keccak256(address));
            let storage_map = reth_trie::HashedStorage::from_iter(
                account_changes
                    .storage_post_states()
                    .map(|(slot, value)| (keccak256(slot.to_be_bytes::<32>()), value)),
            );

            let mut hashed_state = reth_trie::HashedPostState::default();
            hashed_state.storages.insert(hashed_address, storage_map);
            hashed_update_stream.on_hashed_state_update(hashed_state);
        }

        let existing_account = if account_info.is_complete() {
            None
        } else {
            if provider.is_none() {
                let _span = debug_span!(
                    target: "engine::tree::payload_processor::prewarm",
                    parent: parent_span,
                    "bal_hashed_state_provider_init",
                    has_saved_cache = !self.disable_bal_batch_io && self.saved_cache.is_some(),
                )
                .entered();

                let inner = match self.provider.database_provider_ro() {
                    Ok(p) => p,
                    Err(err) => {
                        warn!(
                            target: "engine::tree::payload_processor::prewarm",
                            ?err,
                            "Failed to build provider for BAL account reads"
                        );
                        return;
                    }
                };
                let boxed: Box<dyn AccountReader> =
                    match (self.disable_bal_batch_io, &self.saved_cache) {
                        (false, Some(saved)) => {
                            let caches = saved.cache().clone();
                            Box::new(
                                CachedStateProvider::new_prewarm(inner, caches)
                                    .with_txpool_snapshot(self.env.txpool_snapshot.clone()),
                            )
                        }
                        _ => Box::new(inner),
                    };
                *provider = Some(boxed);
            }
            let account_reader = provider.as_ref().expect("provider just initialized");
            account_reader.basic_account(&address).ok().flatten()
        };

        let mut account = existing_account.unwrap_or_default();
        account.apply_bal_info(account_info);
        let hashed_address = hashed_address.unwrap_or_else(|| keccak256(address));

        // It is possible for the resulting account info to be empty. This can happen when, in the
        // same block:
        // * tx1: A new account is funded
        // * tx2: CREATE2 is called on the new account, SELFDESTRUCT is called within the init code
        //
        // In this case the account will have only balance_changes, one for funding and the second
        // setting balance back to zero. The resulting account is fully empty, we mark it as None
        // with no storage changes to indicate that it should be deleted if nothing else.
        //
        // We assume that if the account info is all zero then it can't have storage, so we don't
        // have to explicitly check for empty storage.
        let account = (!account.is_empty()).then_some(account);

        let mut hashed_state = reth_trie::HashedPostState::default();
        hashed_state.accounts.insert(hashed_address, account);
        hashed_update_stream.on_hashed_state_update(hashed_state);
    }
}

/// Returns [`MultiProofTargetsV2`] for withdrawal addresses.
///
/// Withdrawals only modify account balances (no storage), so the targets contain
/// only account-level entries with empty storage sets.
fn multiproof_targets_from_withdrawals(withdrawals: &[Withdrawal]) -> MultiProofTargetsV2 {
    MultiProofTargetsV2 {
        account_targets: withdrawals.iter().map(|w| keccak256(w.address).into()).collect(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::transaction::Recovered;
    use alloy_eip7928::{AccountChanges, BalanceChange, BlockAccessIndex};
    use alloy_primitives::{address, B256, U256};
    use reth_chainspec::ChainSpec;
    use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
    use reth_evm::{execute::WithTxEnv, TxEnvFor};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::Account;
    use reth_provider::test_utils::MockEthProvider;
    use reth_storage_overlay::OverlayManager;

    #[test]
    fn terminate_event_stops_transaction_execution() {
        let terminate_execution = Arc::new(AtomicBool::new(false));
        let ctx = PrewarmContext {
            env: ExecutionEnv::test_default(),
            evm_config: EthEvmConfig::new(Arc::new(ChainSpec::default())),
            saved_cache: None,
            provider: OverlayStateProviderFactory::new(
                MockEthProvider::default(),
                OverlayManager::default().overlay_builder(B256::ZERO),
            ),
            bal_prewarm_pool: None,
            metrics: PrewarmMetrics::default(),
            cache_metrics: None,
            cache_state_metrics: None,
            terminate_execution: Arc::clone(&terminate_execution),
            executed_tx_index: Arc::new(AtomicUsize::new(0)),
            precompile_cache_disabled: false,
            precompile_cache_map: PrecompileCacheMap::default(),
            disable_bal_parallel_state_root: false,
            disable_bal_batch_io: false,
        };
        let (task, actions_tx) =
            PrewarmCacheTask::new(Runtime::test(), PayloadExecutionCache::default(), ctx);
        actions_tx
            .send(PrewarmTaskEvent::Terminate {
                execution_outcome: None,
                valid_block_rx: mpsc::channel().1,
            })
            .unwrap();

        task.run::<WithTxEnv<TxEnvFor<EthEvmConfig>, Recovered<TransactionSigned>>>(
            PrewarmMode::Skipped,
            actions_tx,
        );

        assert!(terminate_execution.load(Ordering::Relaxed));
    }

    fn test_prewarm_context(
        saved_cache: SavedCache,
        saving_duration: Gauge,
    ) -> PrewarmContext<EthPrimitives, MockEthProvider, EthEvmConfig> {
        PrewarmContext {
            env: ExecutionEnv { hash: B256::repeat_byte(2), ..ExecutionEnv::test_default() },
            evm_config: EthEvmConfig::new(Arc::new(ChainSpec::default())),
            saved_cache: Some(saved_cache),
            provider: OverlayStateProviderFactory::new(
                MockEthProvider::default(),
                OverlayManager::default().overlay_builder(B256::ZERO),
            ),
            bal_prewarm_pool: None,
            metrics: PrewarmMetrics {
                cache_saving_duration: saving_duration,
                ..Default::default()
            },
            cache_metrics: None,
            cache_state_metrics: None,
            terminate_execution: Arc::new(AtomicBool::new(false)),
            executed_tx_index: Arc::new(AtomicUsize::new(0)),
            precompile_cache_disabled: false,
            precompile_cache_map: PrecompileCacheMap::default(),
            disable_bal_parallel_state_root: false,
            disable_bal_batch_io: false,
        }
    }

    fn save_test_cache(
        runtime: &Runtime,
        execution_cache: &PayloadExecutionCache,
        saved_cache: SavedCache,
        state: reth_revm::db::BundleState,
        valid: bool,
        saving_duration: Gauge,
    ) {
        let ctx = test_prewarm_context(saved_cache, saving_duration);
        let (task, _) = PrewarmCacheTask::new(runtime.clone(), execution_cache.clone(), ctx);
        let (valid_tx, valid_rx) = mpsc::channel();
        if valid {
            valid_tx.send(()).unwrap();
        }
        drop(valid_tx);
        task.save_cache(
            Arc::new(BlockExecutionOutput { state, result: Default::default() }),
            valid_rx,
        );
    }

    // Observe the handoff before save_cache returns and drops its local variables. A check after
    // return would miss the window where the saving task still owns an extra reference.
    struct CacheSaveObserver {
        cache: PayloadExecutionCache,
        observed: Arc<AtomicBool>,
    }

    impl metrics::GaugeFn for CacheSaveObserver {
        fn increment(&self, _: f64) {}
        fn decrement(&self, _: f64) {}
        fn set(&self, _: f64) {
            assert!(
                self.cache.get_cache_for(B256::repeat_byte(2)).is_some(),
                "saved cache must be available before the saving task returns"
            );
            self.observed.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn save_cache_releases_warm_cache_before_duration_metric() {
        let runtime = Runtime::test();
        let execution_cache = PayloadExecutionCache::default();
        let saved = SavedCache::new(B256::repeat_byte(1), crate::tree::ExecutionCache::new(1_000));
        let address = address!("0000000000000000000000000000000000000001");
        saved.cache().insert_storage(address, B256::ZERO, Some(U256::from(7)));
        execution_cache.update_with_guard(|slot| *slot = Some(saved.clone()));
        // Keep the drop worker occupied: a queued SavedCache would delay cache reuse.
        let (release_tx, release_rx) = mpsc::channel::<()>();
        runtime.spawn_blocking_named("drop", move || {
            let _ = release_rx.recv();
        });
        let observed = Arc::new(AtomicBool::new(false));
        let observer = Arc::new(CacheSaveObserver {
            cache: execution_cache.clone(),
            observed: observed.clone(),
        });
        save_test_cache(
            &runtime,
            &execution_cache,
            saved,
            Default::default(),
            true,
            Gauge::from_arc(observer),
        );
        assert!(observed.load(Ordering::Relaxed), "save duration was not recorded");
        let saved = execution_cache.get_cache_for(B256::repeat_byte(2)).unwrap();
        assert_eq!(
            saved.cache().get_or_try_insert_storage_with(address, B256::ZERO, || Err(())),
            Ok(reth_execution_cache::CachedStatus::Cached(U256::from(7))),
            "handoff must preserve the warmed contents",
        );
        drop(release_tx);
    }

    #[test]
    fn save_cache_blocks_reuse_while_execution_cache_is_cloned() {
        let runtime = Runtime::test();
        let execution_cache = PayloadExecutionCache::default();
        let saved = SavedCache::new(B256::repeat_byte(1), crate::tree::ExecutionCache::new(1_000));
        execution_cache.update_with_guard(|slot| *slot = Some(saved.clone()));
        let other_cache_clone = saved.cache().clone();

        save_test_cache(&runtime, &execution_cache, saved, Default::default(), true, Gauge::noop());

        assert!(execution_cache.get_cache_for(B256::repeat_byte(2)).is_none());
        drop(other_cache_clone);
        assert!(execution_cache.get_cache_for(B256::repeat_byte(2)).is_some());
    }

    #[derive(Clone, Copy, Debug)]
    enum CacheSlot {
        Empty,
        Shared,
        Distinct,
    }

    // EIP-7702 bytecode preserves its owned buffer, letting us observe actual cache destruction.
    struct CacheDropProbe {
        started: Sender<std::thread::ThreadId>,
        inspected: Receiver<()>,
        result: Sender<bool>,
    }

    impl AsRef<[u8]> for CacheDropProbe {
        fn as_ref(&self) -> &[u8] {
            &[0xef, 0x01, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        }
    }

    impl Drop for CacheDropProbe {
        fn drop(&mut self) {
            let _ = self.started.send(std::thread::current().id());
            // A timeout turns destruction under the mutex into a failure instead of a deadlock.
            let unlocked = self.inspected.recv_timeout(std::time::Duration::from_secs(5)).is_ok();
            let _ = self.result.send(unlocked);
        }
    }

    fn observe_cache_drop(
        saved: &SavedCache,
        execution_cache: &PayloadExecutionCache,
        expect_saved_cache: bool,
    ) -> (Receiver<bool>, std::thread::JoinHandle<std::thread::ThreadId>) {
        let (started_tx, started_rx) = mpsc::channel();
        let (inspected_tx, inspected_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let cache = execution_cache.clone();
        let reader = std::thread::spawn(move || {
            let drop_thread = started_rx.recv().unwrap();
            // Waiting for the mutex must return while the removed cache is still being dropped.
            cache.wait_for_availability();
            assert_eq!(cache.get_cache_for(B256::repeat_byte(2)).is_some(), expect_saved_cache);
            cache.update_with_guard(|slot| assert_eq!(slot.is_some(), expect_saved_cache));
            let _ = inspected_tx.send(());
            drop_thread
        });
        let bytes = alloy_primitives::bytes::Bytes::from_owner(CacheDropProbe {
            started: started_tx,
            inspected: inspected_rx,
            result: result_tx,
        });
        let code = reth_revm::bytecode::Bytecode::new_eip7702_raw(bytes.into()).unwrap();
        saved
            .cache()
            .insert_code(B256::repeat_byte(3), Some(reth_primitives_traits::Bytecode(code)));
        (result_rx, reader)
    }

    #[test]
    fn save_cache_freeing_waits_for_validator_cache_references() {
        use crate::tree::payload_processor::{CacheTaskHandle, PayloadHandle};

        let runtime = Runtime::test();
        let execution_cache = PayloadExecutionCache::default();
        let saved = SavedCache::new(B256::repeat_byte(1), crate::tree::ExecutionCache::new(1_000));
        execution_cache.update_with_guard(|cached| *cached = Some(saved.clone()));
        let (dropped, reader) = observe_cache_drop(&saved, &execution_cache, false);
        let ctx = test_prewarm_context(saved, Gauge::noop());
        let (task, actions_tx) =
            PrewarmCacheTask::new(runtime.clone(), execution_cache.clone(), ctx);
        let mut payload = PayloadHandle {
            prewarm_handle: CacheTaskHandle {
                saved_cache: task.ctx.saved_cache.clone(),
                to_prewarm_task: Some(actions_tx.clone()),
                executed_tx_index: task.ctx.executed_tx_index.clone(),
                cache_metrics: None,
            },
            transactions: crossbeam_channel::never::<(usize, Result<(), ()>)>(),
            _span: Span::none(),
        };
        // The validator keeps this ExecutionCache clone after calling terminate_caching.
        let validator_cache = payload.caches().unwrap();
        let valid_block_tx = payload.terminate_caching(Some(Arc::new(BlockExecutionOutput {
            state: Default::default(),
            result: Default::default(),
        })));
        drop(valid_block_tx);
        let prewarm = runtime.spawn_blocking_named("prewarm", move || {
            task.run::<WithTxEnv<TxEnvFor<EthEvmConfig>, Recovered<TransactionSigned>>>(
                PrewarmMode::Skipped,
                actions_tx,
            );
            std::thread::current().id()
        });
        let prewarm_thread = *prewarm.get();

        execution_cache.update_with_guard(|cached| assert!(cached.is_none()));
        assert_eq!(payload.prewarm_handle.saved_cache.as_ref().unwrap().usage_count(), 2);
        assert!(matches!(dropped.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(payload);
        assert!(matches!(dropped.try_recv(), Err(mpsc::TryRecvError::Empty)));

        drop(validator_cache);
        assert!(dropped.try_recv().expect("last ExecutionCache drop must free the cache"));
        let drop_thread = reader.join().unwrap();
        assert_eq!(drop_thread, std::thread::current().id());
        assert_ne!(drop_thread, prewarm_thread);
    }

    fn assert_save_cache_drops_removed_caches(slot: CacheSlot, valid: bool, insert_error: bool) {
        use reth_revm::db::{AccountStatus, BundleAccount, BundleState};

        let runtime = Runtime::test();
        let execution_cache = PayloadExecutionCache::default();
        let cache_to_save =
            SavedCache::new(B256::repeat_byte(1), crate::tree::ExecutionCache::new(1_000));
        let distinct_previous = matches!(slot, CacheSlot::Distinct).then(|| {
            // The same block hash does not imply the same allocation.
            SavedCache::new(B256::repeat_byte(2), crate::tree::ExecutionCache::new(1_000))
        });
        execution_cache.update_with_guard(|cached| {
            *cached = match slot {
                CacheSlot::Empty => None,
                CacheSlot::Shared => Some(cache_to_save.clone()),
                CacheSlot::Distinct => distinct_previous.clone(),
            };
        });
        let expect_saved_cache = valid && !insert_error;
        let mut drops = Vec::new();
        if let Some(previous) = &distinct_previous {
            drops.push(observe_cache_drop(previous, &execution_cache, expect_saved_cache));
        }
        if !expect_saved_cache {
            drops.push(observe_cache_drop(&cache_to_save, &execution_cache, expect_saved_cache));
        }
        // Retaining this SavedCache would prevent save_cache from freeing the old cache.
        drop(distinct_previous);

        // Cleanup must finish even when the shared background drop worker is occupied.
        let (release_tx, release_rx) = mpsc::channel::<()>();
        runtime.spawn_blocking_named("drop", move || {
            let _ = release_rx.recv();
        });

        let mut state = BundleState::default();
        if insert_error {
            // Modified accounts without current info are rejected by insert_state.
            state.state.insert(
                address!("0000000000000000000000000000000000000001"),
                BundleAccount::new(None, None, Default::default(), AccountStatus::Changed),
            );
        }
        save_test_cache(&runtime, &execution_cache, cache_to_save, state, valid, Gauge::noop());

        for (result, reader) in drops {
            assert!(
                result.try_recv().expect("removed cache must be destroyed before save returns"),
                "cache mutex must be unlocked during destruction"
            );
            assert_eq!(reader.join().unwrap(), std::thread::current().id());
        }
        execution_cache.update_with_guard(|slot| {
            if expect_saved_cache {
                let saved = slot.as_ref().expect("valid cache saved");
                assert_eq!(saved.executed_block_hash(), B256::repeat_byte(2));
            } else {
                assert!(slot.is_none(), "polluted cache must be removed");
            }
        });
        assert_eq!(
            execution_cache.get_cache_for(B256::repeat_byte(2)).is_some(),
            expect_saved_cache
        );
        drop(release_tx);
    }

    #[test]
    fn save_cache_drops_replaced_allocation_after_unlock() {
        assert_save_cache_drops_removed_caches(CacheSlot::Distinct, true, false);
    }

    #[test]
    fn save_cache_drops_invalid_allocations_after_unlock() {
        assert_save_cache_drops_removed_caches(CacheSlot::Distinct, false, false);
    }

    #[test]
    fn save_cache_drops_allocations_after_unlock_on_insert_error() {
        assert_save_cache_drops_removed_caches(CacheSlot::Distinct, true, true);
    }

    #[test]
    fn save_cache_drops_shared_allocation_after_unlock_on_invalid_block() {
        assert_save_cache_drops_removed_caches(CacheSlot::Shared, false, false);
    }

    #[test]
    fn save_cache_drops_shared_allocation_after_unlock_on_insert_error() {
        assert_save_cache_drops_removed_caches(CacheSlot::Shared, true, true);
    }

    #[test]
    fn save_cache_handles_empty_slot() {
        for (valid, insert_error) in [(true, false), (false, false), (true, true)] {
            assert_save_cache_drops_removed_caches(CacheSlot::Empty, valid, insert_error);
        }
    }

    #[test]
    fn bal_read_only_account_does_not_change_state_root() {
        let changes = AccountChanges::new(address!("0000000000000000000000000000000000000001"))
            .with_storage_read(U256::from(1));

        assert!(!changes.account_info().changes_state_root(&changes));
    }

    #[test]
    fn bal_account_uses_existing_fields_only_when_missing() {
        let changes = AccountChanges::new(address!("0000000000000000000000000000000000000001"))
            .with_balance_change(BalanceChange::new(BlockAccessIndex::new(1), U256::from(10)));
        let info = changes.account_info();

        assert!(!info.is_complete());
        let mut account = Account {
            balance: U256::from(1),
            nonce: 3,
            bytecode_hash: Some(B256::repeat_byte(0xaa)),
        };
        account.apply_bal_info(info);

        assert_eq!(account.balance, U256::from(10));
        assert_eq!(account.nonce, 3);
        assert_eq!(account.bytecode_hash, Some(B256::repeat_byte(0xaa)));
    }
}

/// The events the pre-warm task can handle.
///
/// Generic over `R` (receipt type) to allow sharing `Arc<ExecutionOutcome<R>>` with the main
/// execution path without cloning the expensive `BundleState`.
#[derive(Debug)]
pub enum PrewarmTaskEvent<R> {
    /// Signals the prewarm workers to stop executing further transactions.
    ///
    /// This only sets the termination flag the workers poll; the task keeps running to save the
    /// cache. Sent once the authoritative execution no longer needs prewarming, so the workers do
    /// not race ahead on transactions that will never be used.
    TerminateTransactionExecution,
    /// Tears the whole task down: stops the workers, optionally saves the warmed cache from the
    /// final output, and exits.
    ///
    /// Sent when execution completed successfully (carrying the output to save) or when the task
    /// handle is dropped (carrying no output, e.g. after an execution error). Handling this event
    /// also stops the workers, since a teardown may arrive without a preceding
    /// [`TerminateTransactionExecution`](Self::TerminateTransactionExecution).
    Terminate {
        /// The final execution outcome, or `None` when the task is torn down without one (e.g. a
        /// dropped handle). Using `Arc` allows sharing with the main execution path without
        /// cloning the expensive `BundleState`.
        execution_outcome: Option<Arc<BlockExecutionOutput<R>>>,
        /// Receiver for the block validation result.
        ///
        /// Cache saving is racing the state root validation. We optimistically construct the
        /// updated cache but only save it once we know the block is valid.
        valid_block_rx: mpsc::Receiver<()>,
    },
    /// Emitted by the worker-dispatch side once every dispatched transaction has finished or been
    /// cancelled, reporting how many were executed.
    FinishedTxExecution {
        /// Number of transactions executed
        executed_transactions: usize,
    },
}

/// Metrics for transactions prewarming.
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.prewarm")]
pub struct PrewarmMetrics {
    /// The number of transactions to prewarm
    pub(crate) transactions: Gauge,
    /// A histogram of the number of transactions to prewarm
    pub(crate) transactions_histogram: Histogram,
    /// A histogram of duration per transaction prewarming
    pub(crate) total_runtime: Histogram,
    /// A histogram of EVM execution duration per transaction prewarming
    pub(crate) execution_duration: Histogram,
    /// A histogram for prefetch targets per transaction prewarming
    pub(crate) prefetch_storage_targets: Histogram,
    /// Time spent in `save_cache`, including dropping its removed `SavedCache` values.
    /// Excludes any later freeing of cache contents by other `ExecutionCache` clones.
    pub(crate) cache_saving_duration: Gauge,
    /// Counter for transaction execution errors during prewarming
    pub(crate) transaction_errors: Counter,
    /// A histogram of BAL slot iteration duration during prefetching
    pub(crate) bal_slot_iteration_duration: Histogram,
}
