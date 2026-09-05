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
use alloy_primitives::{keccak256, B256, U256};
use metrics::{Counter, Gauge, Histogram};
use rayon::prelude::*;
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm, Evm, EvmFor, RecoveredTx, SpecFor};
use reth_metrics::Metrics;
use reth_primitives_traits::{Account, FastInstant as Instant, NodePrimitives};
use reth_provider::{
    AccountReader, BlockExecutionOutput, BlockNumReader, ChangeSetReader, DatabaseProviderFactory,
    DatabaseProviderROFactory, HistoryReader, PruneCheckpointReader, StageCheckpointReader,
    StateProviderBox, StorageChangeSetReader, StorageSettingsCache,
};
use reth_revm::database::StateProviderDatabase;
use reth_storage_overlay::OverlayStateProviderFactory;
use reth_tasks::{pool::WorkerPool, Runtime, TaskRuntime};
use reth_trie_common::MultiProofTargetsV2;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, channel, Receiver, Sender},
        Arc,
    },
    time::Duration,
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

            Self::transact(ctx, evm, index, tx, state_root_hint_stream);
        });
    }

    /// Shared speculative transaction body. Cooperative callers create and drop the EVM in one
    /// CPU job, so its database reader never crosses a scheduling boundary.
    fn transact<Tx>(
        ctx: &PrewarmContext<N, P, Evm>,
        evm: &mut EvmFor<Evm, StateProviderDatabase<reth_provider::StateProviderBox>>,
        index: usize,
        tx: Tx,
        state_root_hint_stream: Option<&StateRootHintStream>,
    ) where
        Tx: ExecutableTxFor<Evm>,
    {
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
        #[cfg(test)]
        if let Some(transactions) = &ctx.cooperative_transactions {
            transactions.fetch_add(1, Ordering::Relaxed);
        }

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
        self.save_cache_with_validity(execution_outcome, || valid_block_rx.recv().is_ok());
    }

    fn save_cache_with_validity(
        self,
        execution_outcome: Arc<BlockExecutionOutput<N::Receipt>>,
        valid: impl FnOnce() -> bool,
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

                if valid() {
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
                pool.warm_account(
                    account.address,
                    account
                        .storage_changes
                        .iter()
                        .map(|change| change.slot.into())
                        .chain(account.storage_reads.iter().map(|&slot| slot.into())),
                );
            }
            ctx.metrics.bal_slot_iteration_duration.record(dispatch_start.elapsed());
            pool.end_block();
        }

        stream_rx
            .blocking_recv()
            .expect("BAL hashed-state streaming task dropped without signaling completion");

        // Drop the per-thread providers
        executor.bal_streaming_pool().clear();
        executor.prewarming_pool().clear();

        let _ = actions_tx.send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: 0 });
    }

    /// Runs speculative work and the ordinary cache lifecycle on a cooperative runtime.
    pub(crate) async fn run_cooperative<Tx>(
        self,
        runtime: TaskRuntime,
        mode: PrewarmMode<Tx>,
        actions_tx: Sender<PrewarmTaskEvent<N::Receipt>>,
    ) where
        Tx: ExecutableTxFor<Evm> + Send + 'static,
    {
        let ctx = self.ctx.clone();
        let worker_runtime = runtime.clone();
        let producer = runtime
            .spawn("prewarm-dispatch", async move {
                let executed_transactions = match mode {
                    PrewarmMode::Transactions { pending, hints } => {
                        Self::prewarm_transactions_cooperative(ctx, &worker_runtime, pending, hints)
                            .await
                    }
                    PrewarmMode::BlockAccessList { bal, updates } => {
                        Self::prewarm_bal_cooperative(ctx, &worker_runtime, bal, updates).await;
                        0
                    }
                    PrewarmMode::Skipped => 0,
                };
                let _ = actions_tx
                    .send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions });
            })
            .abort_on_drop();

        let mut state = PrewarmRunState::default();
        loop {
            match self.actions_rx.try_recv() {
                Ok(event) => {
                    if self.on_event(&mut state, event) {
                        break;
                    }
                    runtime.yield_now().await;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    runtime.sleep(Duration::from_millis(1)).await;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.ctx.stop();
                    break;
                }
            }
        }
        producer.await.expect("cooperative prewarm producer failed");
        if let Some(Some((outcome, validity))) = state.final_execution_outcome {
            // Waiting with the cache mutex held would prevent validation or another actor from
            // checking out a cache on the simulator's sole execution thread.
            let valid = loop {
                match validity.try_recv() {
                    Ok(()) => break true,
                    Err(mpsc::TryRecvError::Disconnected) => break false,
                    Err(mpsc::TryRecvError::Empty) => {
                        runtime.sleep(Duration::from_millis(1)).await;
                    }
                }
            };
            self.save_cache_with_validity(outcome, || valid);
        }
    }

    async fn prewarm_transactions_cooperative<Tx>(
        ctx: PrewarmContext<N, P, Evm>,
        runtime: &TaskRuntime,
        pending: mpsc::Receiver<(usize, Tx)>,
        hints: Option<StateRootHintStream>,
    ) -> usize
    where
        Tx: ExecutableTxFor<Evm> + Send + 'static,
    {
        let mut workers = std::collections::VecDeque::new();
        let mut count = 0;
        while !ctx.should_stop() {
            let (index, tx) = match pending.try_recv() {
                Ok(transaction) => transaction,
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {
                    runtime.sleep(Duration::from_millis(1)).await;
                    continue;
                }
            };
            if index < ctx.executed_tx_index.load(Ordering::Relaxed) {
                continue;
            }
            let ctx = ctx.clone();
            let hints = hints.clone();
            workers.push_back(
                runtime
                    .spawn_cpu("prewarm-tx", move || {
                        if ctx.should_stop() ||
                            index < ctx.executed_tx_index.load(Ordering::Relaxed)
                        {
                            return;
                        }
                        if let Some(mut evm) = ctx.evm_for_ctx() {
                            Self::transact(&ctx, &mut evm, index, tx, hints.as_ref());
                        }
                    })
                    .abort_on_drop(),
            );
            count += 1;
            // Two independently scheduled jobs expose completion reordering without queuing an
            // unbounded number of speculative EVMs. No EVM survives its individual CPU job.
            if workers.len() == 2 {
                workers.pop_front().unwrap().await.expect("cooperative prewarm transaction failed");
            }
            runtime.yield_now().await;
        }
        if let Some(hints) = &hints &&
            let Some(withdrawals) = &ctx.env.withdrawals &&
            !withdrawals.is_empty()
        {
            hints.on_access_hint(multiproof_targets_from_withdrawals(withdrawals).into());
        }
        for worker in workers {
            worker.await.expect("cooperative prewarm transaction failed");
        }
        count
    }

    async fn prewarm_bal_cooperative(
        ctx: PrewarmContext<N, P, Evm>,
        runtime: &TaskRuntime,
        bal: Arc<DecodedBal>,
        updates: Option<StateRootUpdateStream>,
    ) {
        let stream_ctx = ctx.clone();
        let stream_bal = bal.clone();
        let stream_runtime = runtime.clone();
        let stream = runtime
            .spawn("prewarm-bal-stream", async move {
                if let Some(updates) = updates {
                    let updates = Arc::new(updates);
                    for account in stream_bal.as_bal() {
                        let ctx = stream_ctx.clone();
                        let updates = updates.clone();
                        let account = account.clone();
                        stream_runtime
                            .spawn_cpu("prewarm-bal-account", move || {
                                let mut provider = None;
                                ctx.send_bal_hashed_state(
                                    &Span::current(),
                                    &mut provider,
                                    &account,
                                    &updates,
                                );
                            })
                            .abort_on_drop()
                            .await
                            .expect("cooperative BAL streaming failed");
                    }
                    Arc::try_unwrap(updates).expect("BAL workers released update stream").finish();
                }
            })
            .abort_on_drop();
        if !ctx.disable_bal_batch_io &&
            let Some(saved_cache) = &ctx.saved_cache
        {
            let builder = ctx.provider.clone();
            BalPrewarmPool::prewarm_cooperative(
                runtime,
                Arc::new(move || {
                    builder.database_provider_ro().map(|provider| Box::new(provider) as _)
                }),
                saved_cache.cache().clone(),
                ctx.env.txpool_snapshot.clone(),
                bal,
                ctx.terminate_execution.clone(),
            )
            .await;
        }
        // Authoritative BAL updates must finish even when speculative prefetch has been stopped.
        stream.await.expect("cooperative BAL streaming failed");
    }

    fn on_event(
        &self,
        state: &mut PrewarmRunState<N::Receipt>,
        event: PrewarmTaskEvent<N::Receipt>,
    ) -> bool {
        match event {
            PrewarmTaskEvent::TerminateTransactionExecution => self.ctx.stop(),
            PrewarmTaskEvent::Terminate { execution_outcome, valid_block_rx } => {
                self.ctx.stop();
                state.final_execution_outcome =
                    Some(execution_outcome.map(|outcome| (outcome, valid_block_rx)));
            }
            PrewarmTaskEvent::FinishedTxExecution { executed_transactions } => {
                self.ctx.metrics.transactions.set(executed_transactions as f64);
                self.ctx.metrics.transactions_histogram.record(executed_transactions as f64);
                state.finished_execution = true;
            }
        }
        state.finished_execution && state.final_execution_outcome.is_some()
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

        let mut state = PrewarmRunState::default();
        while let Ok(event) = self.actions_rx.recv() {
            if self.on_event(&mut state, event) {
                break;
            }
        }

        debug!(target: "engine::tree::payload_processor::prewarm", "Completed prewarm execution");

        // save caches and finish using the shared ExecutionOutcome
        if let Some(Some((execution_outcome, valid_block_rx))) = state.final_execution_outcome {
            self.save_cache(execution_outcome, valid_block_rx);
        }
    }
}

type PendingCacheSave<R> = (Arc<BlockExecutionOutput<R>>, mpsc::Receiver<()>);

struct PrewarmRunState<R> {
    final_execution_outcome: Option<Option<PendingCacheSave<R>>>,
    finished_execution: bool,
}

impl<R> Default for PrewarmRunState<R> {
    fn default() -> Self {
        Self { final_execution_outcome: None, finished_execution: false }
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
    /// Optional observer for successful speculative execution on a cooperative runtime.
    #[cfg(test)]
    pub(crate) cooperative_transactions: Option<Arc<AtomicUsize>>,
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
        let account_fields = BalAccountStateFields::from_changes(account_changes);

        if !bal_account_changes_state_root(account_changes, account_fields) {
            return;
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
    use alloy_consensus::transaction::Recovered;
    use alloy_eip7928::{
        AccountChanges, BalanceChange, BlockAccessIndex, CodeChange, NonceChange, SlotChanges,
        StorageChange,
    };
    use alloy_primitives::{address, bytes, Address};
    use reth_chainspec::ChainSpec;
    use reth_ethereum_primitives::TransactionSigned;
    use reth_evm::{execute::WithTxEnv, TxEnvFor};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_provider::test_utils::MockEthProvider;
    use reth_storage_overlay::OverlayManager;

    #[derive(Default)]
    struct ObservedRoot {
        hints: AtomicUsize,
        updates: std::sync::Mutex<Vec<reth_trie::HashedPostState>>,
        finished: AtomicUsize,
    }

    impl super::super::StateRootSink for ObservedRoot {
        fn on_access_hint(&self, hint: super::super::StateAccessHint) {
            let targets: MultiProofTargetsV2 = hint.into();
            assert!(!targets.account_targets.is_empty());
            self.hints.fetch_add(1, Ordering::Relaxed);
        }

        fn on_state_update(&self, _state: revm::state::EvmState) {
            panic!("prewarm workers never emit authoritative EVM state");
        }

        fn on_hashed_state_update(&self, state: reth_trie::HashedPostState) {
            self.updates.lock().unwrap().push(state);
        }

        fn on_updates_finished(&self) {
            self.finished.fetch_add(1, Ordering::Relaxed);
        }
    }

    type TestPrewarmContext = PrewarmContext<
        reth_ethereum_primitives::EthPrimitives,
        reth_provider::ProviderFactory<reth_provider::test_utils::MockNodeTypesWithDB>,
        EthEvmConfig,
    >;

    fn cooperative_context() -> TestPrewarmContext {
        let genesis = serde_json::from_value(serde_json::json!({
            "config": { "chainId": 1 },
            "gasLimit": "0x1c9c380",
            "alloc": {
                "2222222222222222222222222222222222222222": {
                    "balance": "0x0", "nonce": "0x1", "code": "0x6009545000",
                    "storage": {
                        "0x0000000000000000000000000000000000000000000000000000000000000009":
                        "0x0000000000000000000000000000000000000000000000000000000000000002"
                    }
                },
                "3333333333333333333333333333333333333333": {
                    "balance": "0x0", "nonce": "0x1", "code": "0x6009545000",
                    "storage": {
                        "0x0000000000000000000000000000000000000000000000000000000000000009":
                        "0x0000000000000000000000000000000000000000000000000000000000000003"
                    }
                }
            }
        }))
        .unwrap();
        let chain = Arc::new(
            reth_chainspec::ChainSpecBuilder::default()
                .chain(reth_chainspec::MAINNET.chain)
                .genesis(genesis)
                .cancun_activated()
                .build(),
        );
        let provider =
            reth_provider::test_utils::create_test_provider_factory_with_chain_spec(chain.clone());
        let parent_hash = reth_db_common::init::init_genesis(&provider).unwrap();
        let env = ExecutionEnv {
            hash: B256::repeat_byte(0xaa),
            parent_hash,
            ..ExecutionEnv::test_default()
        };
        PrewarmContext {
            env,
            evm_config: EthEvmConfig::new(chain),
            saved_cache: Some(SavedCache::new(
                parent_hash,
                crate::tree::ExecutionCache::new_deterministic(1_000_000),
            )),
            provider: OverlayStateProviderFactory::new(
                provider,
                OverlayManager::default().overlay_builder(parent_hash),
            ),
            bal_prewarm_pool: None,
            metrics: PrewarmMetrics::default(),
            cache_metrics: None,
            cache_state_metrics: None,
            terminate_execution: Arc::new(AtomicBool::new(false)),
            executed_tx_index: Arc::new(AtomicUsize::new(0)),
            precompile_cache_disabled: true,
            precompile_cache_map: PrecompileCacheMap::default(),
            disable_bal_parallel_state_root: false,
            disable_bal_batch_io: false,
            cooperative_transactions: Some(Arc::new(AtomicUsize::new(0))),
        }
    }

    fn speculative_transaction() -> WithTxEnv<TxEnvFor<EthEvmConfig>, Recovered<TransactionSigned>>
    {
        let caller = Address::repeat_byte(0x11);
        let tx = TxEnvFor::<EthEvmConfig> {
            caller,
            kind: alloy_primitives::TxKind::Call(Address::repeat_byte(0x22)),
            gas_limit: 100_000,
            value: U256::from(1),
            ..Default::default()
        };
        let signed = TransactionSigned::new_unhashed(
            alloy_consensus::TxLegacy {
                gas_limit: 100_000,
                to: alloy_primitives::TxKind::Call(Address::repeat_byte(0x22)),
                value: U256::from(1),
                ..Default::default()
            }
            .into(),
            alloy_primitives::Signature::new(U256::from(1), U256::from(1), false),
        );
        WithTxEnv::new((tx, Recovered::new_unchecked(signed, caller)))
    }

    #[test]
    fn deterministic_block_prewarm_cache_lifecycle() {
        use commonware_runtime::{deterministic, Runner, Supervisor};
        let seeds: Vec<u64> = match std::env::var("RETH_DST_SEED") {
            Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
            Err(std::env::VarError::NotPresent) => (0..16).collect(),
            Err(error) => panic!("invalid RETH_DST_SEED: {error}"),
        };
        let simulate = |seed, mode| {
            let ctx = cooperative_context();
            let config = deterministic::Config::default()
                .with_seed(seed)
                .with_timeout(Some(Duration::from_secs(2)));
            deterministic::Runner::new(config).start(|context| async move {
                let runtime = TaskRuntime::deterministic(context.child("block_prewarm"));
                let root = Arc::new(ObservedRoot::default());
                let successful = ctx.cooperative_transactions.clone().unwrap();
                let index = ctx.executed_tx_index.clone();
                let stopped = ctx.terminate_execution.clone();
                let warmed = ctx.saved_cache.as_ref().unwrap().cache().clone();
                let hash = ctx.env.hash;
                let cache = PayloadExecutionCache::default();
                let (task, actions) = PrewarmCacheTask::new(Runtime::test(), cache.clone(), ctx);
                let (pending, transactions) = mpsc::channel();
                let mode_spec = PrewarmMode::Transactions {
                    pending: transactions,
                    hints: Some(StateRootHintStream::new(root.clone())),
                };
                let task_runtime = runtime.clone();
                let task_actions = actions.clone();
                let task = runtime.spawn("cache", async move {
                    task.run_cooperative(task_runtime, mode_spec, task_actions).await;
                });
                pending.send((1, speculative_transaction())).unwrap();
                runtime.sleep(Duration::from_millis(5)).await;
                assert_eq!(successful.load(Ordering::Relaxed), 1);
                assert!(root.hints.load(Ordering::Relaxed) > 0);
                assert!(matches!(
                    warmed
                        .get_or_try_insert_account_with(Address::repeat_byte(0x22), || {
                            Err::<Option<Account>, _>("expected a speculative cache hit")
                        })
                        .unwrap(),
                    reth_execution_cache::CachedStatus::Cached(Some(account)) if account.nonce == 1
                ));
                assert!(matches!(warmed.get_or_try_insert_storage_with(
                    Address::repeat_byte(0x22), B256::from(U256::from(9)), || {
                        Err::<U256, _>("expected speculative SLOAD cache hit")
                    }).unwrap(), reth_execution_cache::CachedStatus::Cached(value) if value == U256::from(2)));
                assert!(matches!(warmed.get_or_try_insert_code_with(keccak256(bytes!("6009545000")), || {
                    Err::<Option<reth_primitives_traits::Bytecode>, _>("expected speculative bytecode cache hit")
                }).unwrap(), reth_execution_cache::CachedStatus::Cached(Some(_))));

                // The distributor was waiting for more input when authoritative execution passed
                // transaction 2. Its resumed worker must observe the new index and skip it.
                index.store(3, Ordering::Relaxed);
                pending.send((2, speculative_transaction())).unwrap();
                runtime.sleep(Duration::from_millis(5)).await;
                assert_eq!(successful.load(Ordering::Relaxed), 1);
                actions.send(PrewarmTaskEvent::TerminateTransactionExecution).unwrap();
                runtime.sleep(Duration::from_millis(2)).await;
                assert!(stopped.load(Ordering::Relaxed));
                drop(pending);
                drop(warmed);

                let (valid, validity) = mpsc::channel();
                actions
                    .send(PrewarmTaskEvent::Terminate {
                        execution_outcome: (mode != 2)
                            .then(|| Arc::new(BlockExecutionOutput::default())),
                        valid_block_rx: validity,
                    })
                    .unwrap();
                runtime.sleep(Duration::from_millis(2)).await;
                // A validity wait must leave this mutex available to other runtime actors.
                cache.update_with_guard(|slot| assert!(slot.is_none()));
                if mode == 0 {
                    valid.send(()).unwrap();
                }
                drop(valid);
                task.await.unwrap();
                assert_eq!(cache.get_cache_for(hash).is_some(), mode == 0);
                assert_eq!(successful.load(Ordering::Relaxed), 1);
                (context.auditor().state(), root.hints.load(Ordering::Relaxed))
            })
        };
        // Native debug EVM frames require more stack than the default Rust test thread.
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                for seed in seeds {
                    for mode in 0..3 {
                        assert_eq!(
                            simulate(seed, mode),
                            simulate(seed, mode),
                            "seed={seed}, mode={mode}"
                        );
                    }
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn deterministic_bal_prewarm_reads_and_streams_state() {
        use commonware_runtime::{deterministic, Runner, Supervisor};
        let simulate = || {
            let ctx = cooperative_context();
            deterministic::Runner::new(deterministic::Config::default().with_seed(7)
                .with_timeout(Some(Duration::from_secs(2))))
                .start(|context| async move {
                    let runtime = TaskRuntime::deterministic(context.child("bal_prewarm"));
                    let address = Address::repeat_byte(0x33);
                    let slot = U256::from(9);
                    let changes = AccountChanges::new(address)
                        .with_balance_change(BalanceChange::new(BlockAccessIndex::new(1), U256::from(10)))
                        .with_storage_change(SlotChanges::new(slot, vec![StorageChange::new(BlockAccessIndex::new(1), U256::from(7))]));
                    let bal: alloy_eip7928::bal::Bal = vec![changes].into();
                    let raw = alloy_rlp::encode(&bal).into();
                    let bal = Arc::new(DecodedBal::new(bal, raw));
                    let root = Arc::new(ObservedRoot::default());
                    let warmed = ctx.saved_cache.as_ref().unwrap().cache().clone();
                    PrewarmCacheTask::prewarm_bal_cooperative(ctx, &runtime, bal,
                        Some(StateRootUpdateStream::new(root.clone()))).await;
                    assert_eq!(root.finished.load(Ordering::Relaxed), 1);
                    assert!(matches!(warmed.get_or_try_insert_storage_with(address, slot.into(), || {
                        Err::<U256, _>("expected BAL storage cache hit")
                    }).unwrap(), reth_execution_cache::CachedStatus::Cached(value) if value == U256::from(3)));
                    let updates = root.updates.lock().unwrap();
                    assert_eq!(updates.len(), 2);
                    let hashed_address = keccak256(address);
                    assert_eq!(updates[0].storages[&hashed_address].storage[&keccak256(slot.to_be_bytes::<32>())], U256::from(7));
                    assert_eq!(updates[1].accounts[&hashed_address].as_ref().unwrap().balance, U256::from(10));
                    context.auditor().state()
                })
        };
        assert_eq!(simulate(), simulate());
    }

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
            cooperative_transactions: None,
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
    /// A histogram of duration for cache saving
    pub(crate) cache_saving_duration: Gauge,
    /// Counter for transaction execution errors during prewarming
    pub(crate) transaction_errors: Counter,
    /// A histogram of BAL slot iteration duration during prefetching
    pub(crate) bal_slot_iteration_duration: Histogram,
}
