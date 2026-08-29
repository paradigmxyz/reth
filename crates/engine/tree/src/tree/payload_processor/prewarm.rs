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
    PayloadExecutionCache, SavedCache, StateProviderBuilder,
};
use alloy_consensus::transaction::TxHashRef;
use alloy_eip7928::bal::DecodedBal;
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{keccak256, B256, U256};
use metrics::{Counter, Gauge, Histogram};
use rayon::prelude::*;
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm, Evm, EvmFor, RecoveredTx, SpecFor};
use reth_metrics::Metrics;
use reth_primitives_traits::{Account, FastInstant as Instant, NodePrimitives};
use reth_provider::{
    AccountReader, BlockExecutionOutput, BlockNumReader, DatabaseProviderFactory, ProviderResult,
    PruneCheckpointReader, StageCheckpointReader, StorageSettingsCache,
    TryIntoHistoricalStateProvider,
};
use reth_revm::database::StateProviderDatabase;
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
        + StorageSettingsCache
        + TryIntoHistoricalStateProvider
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

    /// This method calls `ExecutionCache::update_with_guard` which requires exclusive access.
    /// It should only be called after ensuring that:
    /// 1. All prewarming tasks have completed execution
    /// 2. No other concurrent operations are accessing the cache
    ///
    /// Saves the warmed caches back into the shared slot after prewarming completes.
    ///
    /// This consumes the `SavedCache` held by the task, which releases its cache handle and allows
    /// the new, warmed cache to be inserted.
    ///
    /// This method is called from `run()` only after all execution tasks are complete.
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
            execution_cache.update_with_guard(|cached| {
                // consumes the `SavedCache` held by the prewarming task, which releases its cache
                // handle
                let caches = saved_cache.cache().clone();
                let new_cache = SavedCache::new(hash, caches);

                // Insert state into cache while holding the lock
                // Access the BundleState through the shared ExecutionOutcome
                if new_cache.cache().insert_state(&execution_outcome.state).is_err() {
                    // Clear the cache on error to prevent having a polluted cache
                    *cached = None;
                    debug!(target: "engine::caching", "cleared execution cache on update error");
                    return;
                }

                new_cache.update_metrics(cache_state_metrics.as_ref());

                if valid_block_rx.recv().is_ok() {
                    // Replace the shared cache with the new one; the previous cache (if any) is
                    // dropped.
                    *cached = Some(new_cache);
                } else {
                    // Block was invalid; caches were already mutated by insert_state above,
                    // so we must clear to prevent using polluted state
                    *cached = None;
                    debug!(target: "engine::caching", "cleared execution cache on invalid block");
                }
            });

            let elapsed = start.elapsed();
            debug!(target: "engine::caching", parent_hash=?hash, elapsed=?elapsed, "Updated execution cache");

            metrics.cache_saving_duration.set(elapsed.as_secs_f64());
        }
    }

    /// Runs BAL-based prewarming and state-root streaming.
    ///
    /// Spawns two halves concurrently on separate pools, then waits for both to complete:
    /// 1. Hashed state streaming on the BAL streaming pool so storage updates can reach the
    ///    state-root job before account reads finish.
    /// 2. Storage prefetch on the prewarming pool to populate the execution cache, unless BAL batch
    ///    I/O is disabled.
    ///
    /// Both halves stop early once the block is cancelled. The update stream is then dropped
    /// unfinished, so the state-root task cannot compute a root from partial updates.
    #[instrument(level = "debug", target = "engine::tree::payload_processor::prewarm", skip_all)]
    fn run_bal_prewarm(
        ctx: PrewarmContext<N, P, Evm>,
        executor: Runtime,
        decoded_bal: Arc<DecodedBal>,
        actions_tx: Sender<PrewarmTaskEvent<N::Receipt>>,
        hashed_update_stream: Option<StateRootUpdateStream>,
    ) {
        let bal = decoded_bal.as_bal();
        if bal.is_empty() {
            if let Some(hashed_update_stream) = hashed_update_stream {
                if ctx.is_cancelled() {
                    drop(hashed_update_stream);
                } else {
                    hashed_update_stream.finish();
                }
            }
            if !ctx.is_cancelled() {
                let _ = actions_tx
                    .send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: 0 });
            }
            return;
        }

        // Clear per-thread providers on every exit, including unwinding from either prewarm half.
        let pool_cleanup = PrewarmPoolCleanup(executor.clone());

        trace!(
            target: "engine::tree::payload_processor::prewarm",
            accounts = bal.len(),
            "Starting BAL prewarm"
        );

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
                    if ctx.is_cancelled() {
                        return;
                    }
                    WorkerPool::with_worker_mut(|worker| {
                        let provider =
                            worker.get_or_init::<Option<Box<dyn AccountReader>>>(|| None);
                        if let Err(err) = ctx.send_bal_hashed_state(
                            &parent_span,
                            provider,
                            account_changes,
                            &hashed_update_stream,
                        ) {
                            warn!(
                                target: "engine::tree::payload_processor::prewarm",
                                ?err,
                                "Failed to build complete BAL hashed-state stream"
                            );
                            ctx.cancel();
                        }
                    });
                });

                if ctx.is_cancelled() {
                    // Leave the stream unfinished: the updates are incomplete and the state-root
                    // task must not compute a root from them.
                    drop(hashed_update_stream);
                } else {
                    hashed_update_stream.finish();
                }
                let _ = stream_tx.send(());
            });
        } else {
            let _ = stream_tx.send(());
        }

        if let Some(saved_cache) = &ctx.saved_cache &&
            !ctx.disable_bal_batch_io &&
            !ctx.is_cancelled() &&
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
            let provider_builder = ctx.provider.clone();
            let build = Arc::new(move || provider_builder.build());

            let block = pool.begin_block(
                build,
                caches,
                ctx.env.txpool_snapshot.clone(),
                ctx.cancelled.clone(),
            );
            'accounts: for account in prefetch_bal.as_bal() {
                if ctx.is_cancelled() {
                    break;
                }
                block.warm_account(account.address);
                for change in &account.storage_changes {
                    if ctx.is_cancelled() {
                        break 'accounts;
                    }
                    block.warm_storage(account.address, change.slot.into());
                }
                for &slot in &account.storage_reads {
                    if ctx.is_cancelled() {
                        break 'accounts;
                    }
                    block.warm_storage(account.address, slot.into());
                }
            }
            if !block.finish() {
                ctx.cancel();
            }
        }

        if stream_rx.blocking_recv().is_err() {
            warn!(
                target: "engine::tree::payload_processor::prewarm",
                "BAL hashed-state streaming task dropped without signaling completion"
            );
            ctx.cancel();
        }

        // Drop the per-thread providers
        drop(pool_cleanup);

        if !ctx.is_cancelled() {
            let _ =
                actions_tx.send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: 0 });
        }
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
                let ctx = self.ctx.clone();
                let executor = self.executor.clone();
                let span = Span::current();
                self.executor.spawn_blocking_named("prewarm-bal", move || {
                    let _enter = span.entered();
                    Self::run_bal_prewarm(ctx, executor, bal, actions_tx, updates);
                });
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
                    // No outcome means the handle was dropped without one: the block was
                    // abandoned and the BAL hashed-state stream is no longer needed either.
                    if execution_outcome.is_none() {
                        self.ctx.cancel();
                    }
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

        if !finished_execution {
            // The producer disappeared without its completion marker, for example after a panic.
            // Do not publish a cache from a run that skipped its cleanup/completion barrier.
            self.ctx.cancel();
        }

        // save caches and finish using the shared ExecutionOutcome
        if finished_execution &&
            let Some(Some((execution_outcome, valid_block_rx))) = final_execution_outcome
        {
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
    pub provider: StateProviderBuilder<N, P>,
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
    /// An atomic bool set once the block is abandoned, which also aborts the BAL hashed-state
    /// stream. See [`Self::cancel`].
    pub cancelled: Arc<AtomicBool>,
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
        + StorageSettingsCache
        + TryIntoHistoricalStateProvider
        + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    /// Creates a per-thread EVM for prewarming.
    #[instrument(level = "debug", target = "engine::tree::payload_processor::prewarm", skip_all)]
    fn evm_for_ctx(&self) -> PrewarmEvmState<Evm> {
        let mut state_provider = match self.provider.build() {
            Ok(provider) => provider,
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

    /// Returns `true` if the block was abandoned and remaining BAL work should be dropped.
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Signals that the block was abandoned.
    ///
    /// Unlike [`Self::stop`], which is also called once execution completes and only ends
    /// speculative transaction prewarming, this aborts the BAL hashed-state stream the state-root
    /// task depends on, so it must only be set once the state root is no longer needed.
    #[inline]
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
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
    ) -> ProviderResult<()> {
        if self.disable_bal_parallel_state_root {
            return Ok(())
        }
        let address = account_changes.address;
        let mut hashed_address = None;
        let account_fields = BalAccountStateFields::from_changes(account_changes);

        if !bal_account_changes_state_root(account_changes, account_fields) {
            return Ok(())
        }

        // If there are any storage changes we can assume that the resulting account info will be
        // non-empty, so the account will exist, and therefore we can pre-emptively send out storage
        // changes to start processing them before potentially hitting the db in the next step.
        if !account_changes.storage_changes.is_empty() {
            let hashed_address = *hashed_address.get_or_insert_with(|| keccak256(address));
            let mut storage_map = reth_trie::HashedStorage::default();

            for slot_changes in &account_changes.storage_changes {
                let hashed_slot = keccak256(slot_changes.slot.to_be_bytes::<32>());
                if let Some(last_change) = slot_changes.changes.last() {
                    storage_map.storage.insert(hashed_slot, last_change.new_value);
                }
            }

            let mut hashed_state = reth_trie::HashedPostState::default();
            hashed_state.storages.insert(hashed_address, storage_map);
            hashed_update_stream.on_hashed_state_update(hashed_state);
        }

        let existing_account = if account_fields.needs_parent_account() {
            if provider.is_none() {
                let _span = debug_span!(
                    target: "engine::tree::payload_processor::prewarm",
                    parent: parent_span,
                    "bal_hashed_state_provider_init",
                    has_saved_cache = !self.disable_bal_batch_io && self.saved_cache.is_some(),
                )
                .entered();

                let inner = self.provider.build()?;
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
            account_reader.basic_account(&address)?
        } else {
            None
        };

        let account = account_fields.into_account(existing_account);
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
        Ok(())
    }
}

/// Clears worker-local BAL providers before the producer publishes its completion marker.
struct PrewarmPoolCleanup(Runtime);

impl Drop for PrewarmPoolCleanup {
    fn drop(&mut self) {
        self.0.bal_streaming_pool().clear();
        self.0.prewarming_pool().clear();
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BalAccountStateFields {
    balance: Option<U256>,
    nonce: Option<u64>,
    code_hash: Option<B256>,
}

impl BalAccountStateFields {
    fn from_changes(account_changes: &alloy_eip7928::AccountChanges) -> Self {
        Self {
            balance: account_changes.balance_changes.last().map(|change| change.post_balance),
            nonce: account_changes.nonce_changes.last().map(|change| change.new_nonce),
            code_hash: account_changes.code_changes.last().map(|code_change| {
                if code_change.new_code.is_empty() {
                    alloy_consensus::constants::KECCAK_EMPTY
                } else {
                    keccak256(&code_change.new_code)
                }
            }),
        }
    }

    const fn is_empty(self) -> bool {
        self.balance.is_none() && self.nonce.is_none() && self.code_hash.is_none()
    }

    const fn needs_parent_account(self) -> bool {
        self.balance.is_none() || self.nonce.is_none() || self.code_hash.is_none()
    }

    fn into_account(self, existing_account: Option<Account>) -> Account {
        let existing_account = existing_account.as_ref();
        Account {
            balance: self.balance.unwrap_or_else(|| {
                existing_account
                    .map(|account| account.balance)
                    .unwrap_or(alloy_primitives::U256::ZERO)
            }),
            nonce: self
                .nonce
                .unwrap_or_else(|| existing_account.map(|account| account.nonce).unwrap_or(0)),
            bytecode_hash: self.code_hash.or_else(|| {
                existing_account
                    .and_then(|account| account.bytecode_hash)
                    .or(Some(alloy_consensus::constants::KECCAK_EMPTY))
            }),
        }
    }
}

const fn bal_account_changes_state_root(
    account_changes: &alloy_eip7928::AccountChanges,
    account_fields: BalAccountStateFields,
) -> bool {
    !account_fields.is_empty() || !account_changes.storage_changes.is_empty()
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
    use crate::tree::{payload_processor::StateRootSink, ExecutionCache};
    use alloy_consensus::transaction::Recovered;
    use alloy_eip7928::{
        bal::Bal, AccountChanges, BalanceChange, BlockAccessIndex, CodeChange, NonceChange,
        SlotChanges, StorageChange,
    };
    use alloy_primitives::{address, bytes, Address};
    use reth_chainspec::ChainSpec;
    use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
    use reth_evm::{execute::WithTxEnv, TxEnvFor};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_provider::test_utils::MockEthProvider;
    use reth_stages_api::{StageCheckpoint, StageId};
    use reth_storage_overlay::OverlayManager;
    use reth_trie::HashedPostState;
    use revm::state::EvmState;

    /// Builds a prewarm context with the default BAL batch-I/O path enabled.
    fn test_ctx(
        terminate_execution: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
    ) -> PrewarmContext<EthPrimitives, MockEthProvider, EthEvmConfig> {
        let provider = MockEthProvider::default();
        provider.enable_database_provider();
        provider.add_header(B256::ZERO, Default::default());
        provider.add_stage_checkpoint(StageId::Finish, StageCheckpoint::new(0));

        PrewarmContext {
            env: ExecutionEnv::test_default(),
            evm_config: EthEvmConfig::new(Arc::new(ChainSpec::default())),
            saved_cache: Some(SavedCache::new(B256::ZERO, ExecutionCache::new(1_000))),
            provider: StateProviderBuilder::<EthPrimitives, _>::new(
                provider,
                B256::ZERO,
                OverlayManager::default(),
            ),
            bal_prewarm_pool: Some(BalPrewarmPool::new(1)),
            metrics: PrewarmMetrics::default(),
            cache_metrics: None,
            cache_state_metrics: None,
            terminate_execution,
            cancelled,
            executed_tx_index: Arc::new(AtomicUsize::new(0)),
            precompile_cache_disabled: false,
            precompile_cache_map: PrecompileCacheMap::default(),
            disable_bal_parallel_state_root: false,
            disable_bal_batch_io: false,
        }
    }

    /// Records what a BAL prewarm run streams to the state-root task.
    #[derive(Default)]
    struct CountingSink {
        updates: AtomicUsize,
        finished: AtomicBool,
    }

    impl StateRootSink for CountingSink {
        fn on_state_update(&self, _state: EvmState) {}

        fn on_hashed_state_update(&self, _state: HashedPostState) {
            self.updates.fetch_add(1, Ordering::Relaxed);
        }

        fn on_updates_finished(&self) {
            self.finished.store(true, Ordering::Relaxed);
        }
    }

    /// Runs BAL prewarm over `accounts` accounts whose changes carry every leaf field, so no
    /// parent reads are needed, and returns how many hashed-state updates were streamed and
    /// whether the stream was finished.
    fn run_bal_prewarm_to_sink(accounts: u64, cancelled: bool) -> (usize, bool) {
        let mut changes: Vec<AccountChanges> = (0..accounts)
            .map(|i| {
                AccountChanges::new(Address::from_word(keccak256(i.to_be_bytes())))
                    .with_balance_change(BalanceChange::new(
                        BlockAccessIndex::new(1),
                        U256::from(i + 1),
                    ))
                    .with_nonce_change(NonceChange::new(BlockAccessIndex::new(1), 1))
                    .with_code_change(CodeChange::new(
                        BlockAccessIndex::new(1),
                        bytes!("6001600155"),
                    ))
            })
            .collect();
        changes.sort_by_key(|account| account.address);
        let bal = Bal::from(changes);
        let raw = alloy_rlp::encode(&bal).into();
        let bal = Arc::new(DecodedBal::new(bal, raw));

        let sink = Arc::new(CountingSink::default());
        let updates = StateRootUpdateStream::new(sink.clone());
        let ctx = test_ctx(Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(cancelled)));
        let runtime = Runtime::test();
        let (task, actions_tx) =
            PrewarmCacheTask::new(runtime.clone(), PayloadExecutionCache::default(), ctx);

        // The producer drops the last event sender once it has streamed the BAL, which ends the
        // event loop without an explicit terminate.
        task.run::<WithTxEnv<TxEnvFor<EthEvmConfig>, Recovered<TransactionSigned>>>(
            PrewarmMode::BlockAccessList { bal, updates: Some(updates) },
            actions_tx,
        );
        // Named blocking tasks run serially, so this returns once the producer thread is done.
        runtime.spawn_blocking_named("prewarm-bal", || {}).get();

        (sink.updates.load(Ordering::Relaxed), sink.finished.load(Ordering::Relaxed))
    }

    #[test]
    fn terminate_event_stops_transaction_execution() {
        let terminate_execution = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let ctx = test_ctx(Arc::clone(&terminate_execution), Arc::clone(&cancelled));
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
        // A teardown without an outcome means the block was abandoned.
        assert!(cancelled.load(Ordering::Relaxed));
    }

    #[test]
    fn cancelled_bal_prewarm_leaves_stream_unfinished() {
        const ACCOUNTS: u64 = 64;

        let (updates, finished) = run_bal_prewarm_to_sink(ACCOUNTS, false);
        assert_eq!(updates, ACCOUNTS as usize);
        assert!(finished);

        let (updates, finished) = run_bal_prewarm_to_sink(ACCOUNTS, true);
        assert_eq!(updates, 0);
        assert!(!finished, "a cancelled stream must not be finished");
    }

    #[test]
    fn bal_provider_error_leaves_stream_unfinished() {
        let changes = AccountChanges::new(address!("0000000000000000000000000000000000000001"))
            .with_storage_change(SlotChanges::new(
                U256::from(1),
                vec![StorageChange::new(BlockAccessIndex::new(1), U256::from(2))],
            ));
        let bal = Bal::from(vec![changes]);
        let raw = alloy_rlp::encode(&bal).into();
        let bal = Arc::new(DecodedBal::new(bal, raw));

        let sink = Arc::new(CountingSink::default());
        let updates = StateRootUpdateStream::new(sink.clone());
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut ctx = test_ctx(Arc::new(AtomicBool::new(false)), Arc::clone(&cancelled));
        // This mock rejects database-provider creation, forcing the parent account read to fail.
        ctx.provider = StateProviderBuilder::new(
            MockEthProvider::default(),
            B256::ZERO,
            OverlayManager::default(),
        );
        let runtime = Runtime::test();
        let (task, actions_tx) =
            PrewarmCacheTask::new(runtime.clone(), PayloadExecutionCache::default(), ctx);

        task.run::<WithTxEnv<TxEnvFor<EthEvmConfig>, Recovered<TransactionSigned>>>(
            PrewarmMode::BlockAccessList { bal, updates: Some(updates) },
            actions_tx,
        );
        runtime.spawn_blocking_named("prewarm-bal", || {}).get();

        assert!(cancelled.load(Ordering::Relaxed));
        assert!(!sink.finished.load(Ordering::Relaxed));
    }

    #[test]
    fn bal_read_only_account_does_not_change_state_root() {
        let changes = AccountChanges::new(address!("0000000000000000000000000000000000000001"))
            .with_storage_read(U256::from(1));
        let fields = BalAccountStateFields::from_changes(&changes);

        assert!(fields.is_empty());
        assert!(!bal_account_changes_state_root(&changes, fields));
    }

    #[test]
    fn bal_account_with_all_leaf_fields_does_not_need_parent_account() {
        let changes = AccountChanges::new(address!("0000000000000000000000000000000000000001"))
            .with_balance_change(BalanceChange::new(BlockAccessIndex::new(1), U256::from(10)))
            .with_nonce_change(NonceChange::new(BlockAccessIndex::new(1), 7))
            .with_code_change(CodeChange::new(BlockAccessIndex::new(1), bytes!("6001600155")));
        let fields = BalAccountStateFields::from_changes(&changes);

        assert!(bal_account_changes_state_root(&changes, fields));
        assert!(!fields.needs_parent_account());
    }

    #[test]
    fn bal_storage_change_needs_parent_account_when_leaf_fields_missing() {
        let changes = AccountChanges::new(address!("0000000000000000000000000000000000000001"))
            .with_storage_change(SlotChanges::new(
                U256::from(1),
                vec![StorageChange::new(BlockAccessIndex::new(1), U256::from(2))],
            ));
        let fields = BalAccountStateFields::from_changes(&changes);

        assert!(bal_account_changes_state_root(&changes, fields));
        assert!(fields.needs_parent_account());
    }

    #[test]
    fn bal_account_uses_existing_fields_only_when_missing() {
        let changes = AccountChanges::new(address!("0000000000000000000000000000000000000001"))
            .with_balance_change(BalanceChange::new(BlockAccessIndex::new(1), U256::from(10)));
        let fields = BalAccountStateFields::from_changes(&changes);
        let account = fields.into_account(Some(Account {
            balance: U256::from(1),
            nonce: 3,
            bytecode_hash: Some(B256::repeat_byte(0xaa)),
        }));

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
    /// [`TerminateTransactionExecution`](Self::TerminateTransactionExecution). Without an output
    /// the block was abandoned, so the BAL hashed-state stream is cancelled as well.
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
    /// A histogram of duration for cache saving
    pub(crate) cache_saving_duration: Gauge,
    /// Counter for transaction execution errors during prewarming
    pub(crate) transaction_errors: Counter,
    /// A histogram of BAL slot iteration duration during prefetching
    pub(crate) bal_slot_iteration_duration: Histogram,
}
