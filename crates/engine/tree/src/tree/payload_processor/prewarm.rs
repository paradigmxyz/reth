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

use crate::tree::{
    cached_state::{CachedStateProvider, SavedCache},
    payload_processor::{
        bal::{total_slots, BALSlotIter},
        executor::WorkloadExecutor,
        multiproof::MultiProofMessage,
        ExecutionCache as PayloadExecutionCache,
    },
    precompile_cache::{CachedPrecompile, PrecompileCacheMap},
    ExecutionEnv, StateProviderBuilder,
};
use alloy_consensus::transaction::TxHashRef;
use alloy_eip7928::BlockAccessList;
use alloy_eips::Typed2718;
use alloy_evm::Database;
use alloy_primitives::{keccak256, map::B256Set, B256};
use crossbeam_channel::Sender as CrossbeamSender;
use metrics::{Counter, Gauge, Histogram};
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm, Evm, EvmFor, SpecFor};
use reth_execution_types::ExecutionOutcome;
use reth_metrics::Metrics;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{AccountReader, BlockReader, StateProvider, StateProviderFactory, StateReader};
use reth_revm::{database::StateProviderDatabase, state::EvmState};
use reth_trie::MultiProofTargets;
use std::{
    ops::Range,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, channel, Receiver, Sender},
        Arc,
    },
    time::Instant,
};
use tracing::{debug, debug_span, instrument, trace, warn, Span};

/// Determines the prewarming mode: transaction-based or BAL-based.
pub(super) enum PrewarmMode<Tx> {
    /// Prewarm by executing transactions from a stream.
    Transactions(Receiver<Tx>),
    /// Prewarm by prefetching slots from a Block Access List.
    BlockAccessList(Arc<BlockAccessList>),
}

/// A wrapper for transactions that includes their index in the block.
#[derive(Clone)]
struct IndexedTransaction<Tx> {
    /// The transaction index in the block.
    index: usize,
    /// The wrapped transaction.
    tx: Tx,
}

/// Maximum standard Ethereum transaction type value.
///
/// Standard transaction types are:
/// - Type 0: Legacy transactions (original Ethereum)
/// - Type 1: EIP-2930 (access list transactions)
/// - Type 2: EIP-1559 (dynamic fee transactions)
/// - Type 3: EIP-4844 (blob transactions)
/// - Type 4: EIP-7702 (set code authorization transactions)
///
/// Any transaction with a type > 4 is considered a non-standard/system transaction,
/// typically used by L2s for special purposes (e.g., Optimism deposit transactions use type 126).
const MAX_STANDARD_TX_TYPE: u8 = 4;

/// A task that is responsible for caching and prewarming the cache by executing transactions
/// individually in parallel.
///
/// Note: This task runs until cancelled externally.
pub(super) struct PrewarmCacheTask<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    /// The executor used to spawn execution tasks.
    executor: WorkloadExecutor,
    /// Shared execution cache.
    execution_cache: PayloadExecutionCache,
    /// Context provided to execution tasks
    ctx: PrewarmContext<N, P, Evm>,
    /// How many transactions should be executed in parallel
    max_concurrency: usize,
    /// The number of transactions to be processed
    transaction_count_hint: usize,
    /// Sender to emit evm state outcome messages, if any.
    to_multi_proof: Option<CrossbeamSender<MultiProofMessage>>,
    /// Receiver for events produced by tx execution
    actions_rx: Receiver<PrewarmTaskEvent<N::Receipt>>,
    /// Parent span for tracing
    parent_span: Span,
}

impl<N, P, Evm> PrewarmCacheTask<N, P, Evm>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    /// Initializes the task with the given transactions pending execution
    pub(super) fn new(
        executor: WorkloadExecutor,
        execution_cache: PayloadExecutionCache,
        ctx: PrewarmContext<N, P, Evm>,
        to_multi_proof: Option<CrossbeamSender<MultiProofMessage>>,
        transaction_count_hint: usize,
        max_concurrency: usize,
    ) -> (Self, Sender<PrewarmTaskEvent<N::Receipt>>) {
        let (actions_tx, actions_rx) = channel();

        trace!(
            target: "engine::tree::payload_processor::prewarm",
            max_concurrency,
            transaction_count_hint,
            "Initialized prewarm task"
        );

        (
            Self {
                executor,
                execution_cache,
                ctx,
                max_concurrency,
                transaction_count_hint,
                to_multi_proof,
                actions_rx,
                parent_span: Span::current(),
            },
            actions_tx,
        )
    }

    /// Spawns all pending transactions as blocking tasks by first chunking them.
    ///
    /// For Optimism chains, special handling is applied to the first transaction if it's a
    /// deposit transaction (type 0x7E/126) which sets critical metadata that affects all
    /// subsequent transactions in the block.
    fn spawn_all<Tx>(
        &self,
        pending: mpsc::Receiver<Tx>,
        actions_tx: Sender<PrewarmTaskEvent<N::Receipt>>,
    ) where
        Tx: ExecutableTxFor<Evm> + Clone + Send + 'static,
    {
        let executor = self.executor.clone();
        let ctx = self.ctx.clone();
        let max_concurrency = self.max_concurrency;
        let transaction_count_hint = self.transaction_count_hint;
        let span = Span::current();

        self.executor.spawn_blocking(move || {
            let _enter = debug_span!(target: "engine::tree::payload_processor::prewarm", parent: span, "spawn_all").entered();

            let (done_tx, done_rx) = mpsc::channel();

            // When transaction_count_hint is 0, it means the count is unknown. In this case, spawn
            // max workers to handle potentially many transactions in parallel rather
            // than bottlenecking on a single worker.
            let workers_needed = if transaction_count_hint == 0 {
                max_concurrency
            } else {
                transaction_count_hint.min(max_concurrency)
            };

            // Initialize worker handles container
            let handles = ctx.clone().spawn_workers(workers_needed, &executor, actions_tx.clone(), done_tx.clone());

            // Distribute transactions to workers
            let mut tx_index = 0usize;
            while let Ok(tx) = pending.recv() {
                // Stop distributing if termination was requested
                if ctx.terminate_execution.load(Ordering::Relaxed) {
                    trace!(
                        target: "engine::tree::payload_processor::prewarm",
                        "Termination requested, stopping transaction distribution"
                    );
                    break;
                }

                let indexed_tx = IndexedTransaction { index: tx_index, tx };
                let is_system_tx = indexed_tx.tx.tx().ty() > MAX_STANDARD_TX_TYPE;

                // System transactions (type > 4) in the first position set critical metadata
                // that affects all subsequent transactions (e.g., L1 block info on L2s).
                // Broadcast the first system transaction to all workers to ensure they have
                // the critical state. This is particularly important for L2s like Optimism
                // where the first deposit transaction (type 126) contains essential block metadata.
                if tx_index == 0 && is_system_tx {
                    for handle in &handles {
                        // Ignore send errors: workers listen to terminate_execution and may
                        // exit early when signaled. Sending to a disconnected worker is
                        // possible and harmless and should happen at most once due to
                        //  the terminate_execution check above.
                        let _ = handle.send(indexed_tx.clone());
                    }
                } else {
                    // Round-robin distribution for all other transactions
                    let worker_idx = tx_index % workers_needed;
                    // Ignore send errors: workers listen to terminate_execution and may
                    // exit early when signaled. Sending to a disconnected worker is
                    // possible and harmless and should happen at most once due to
                    //  the terminate_execution check above.
                    let _ = handles[worker_idx].send(indexed_tx);
                }

                tx_index += 1;
            }

            // drop handle and wait for all tasks to finish and drop theirs
            drop(done_tx);
            drop(handles);
            while done_rx.recv().is_ok() {}

            let _ = actions_tx
                .send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: tx_index });
        });
    }

    /// Returns true if prewarming was terminated and no more transactions should be prewarmed.
    fn is_execution_terminated(&self) -> bool {
        self.ctx.terminate_execution.load(Ordering::Relaxed)
    }

    /// If configured and the tx returned proof targets, emit the targets the transaction produced
    fn send_multi_proof_targets(&self, targets: Option<MultiProofTargets>) {
        if self.is_execution_terminated() {
            // if execution is already terminated then we dont need to send more proof fetch
            // messages
            return
        }

        if let Some((proof_targets, to_multi_proof)) = targets.zip(self.to_multi_proof.as_ref()) {
            let _ = to_multi_proof.send(MultiProofMessage::PrefetchProofs(proof_targets));
        }
    }

    /// This method calls `ExecutionCache::update_with_guard` which requires exclusive access.
    /// It should only be called after ensuring that:
    /// 1. All prewarming tasks have completed execution
    /// 2. No other concurrent operations are accessing the cache
    ///
    /// Saves the warmed caches back into the shared slot after prewarming completes.
    ///
    /// This consumes the `SavedCache` held by the task, which releases its usage guard and allows
    /// the new, warmed cache to be inserted.
    ///
    /// This method is called from `run()` only after all execution tasks are complete.
    #[instrument(level = "debug", target = "engine::tree::payload_processor::prewarm", skip_all)]
    fn save_cache(self, execution_outcome: Arc<ExecutionOutcome<N::Receipt>>) {
        let start = Instant::now();

        let Self { execution_cache, ctx: PrewarmContext { env, metrics, saved_cache, .. }, .. } =
            self;
        let hash = env.hash;

        if let Some(saved_cache) = saved_cache {
            debug!(target: "engine::caching", parent_hash=?hash, "Updating execution cache");
            // Perform all cache operations atomically under the lock
            execution_cache.update_with_guard(|cached| {
                // consumes the `SavedCache` held by the prewarming task, which releases its usage
                // guard
                let (caches, cache_metrics, fixed_cache_metrics) = saved_cache.split();
                let new_cache = SavedCache::new(hash, caches, cache_metrics, fixed_cache_metrics);

                // Insert state into cache while holding the lock
                // Access the BundleState through the shared ExecutionOutcome
                if new_cache.cache().insert_state(execution_outcome.state()).is_err() {
                    // Clear the cache on error to prevent having a polluted cache
                    *cached = None;
                    debug!(target: "engine::caching", "cleared execution cache on update error");
                    return;
                }

                new_cache.update_metrics();

                // Replace the shared cache with the new one; the previous cache (if any) is
                // dropped.
                *cached = Some(new_cache);
            });

            let elapsed = start.elapsed();
            debug!(target: "engine::caching", parent_hash=?hash, elapsed=?elapsed, "Updated execution cache");

            metrics.cache_saving_duration.set(elapsed.as_secs_f64());
        }
    }

    /// Runs BAL-based prewarming by spawning workers to prefetch storage slots.
    ///
    /// Divides the total slots across `max_concurrency` workers, each responsible for
    /// prefetching a range of slots from the BAL.
    #[instrument(level = "debug", target = "engine::tree::payload_processor::prewarm", skip_all)]
    fn run_bal_prewarm(
        &self,
        bal: Arc<BlockAccessList>,
        actions_tx: Sender<PrewarmTaskEvent<N::Receipt>>,
    ) {
        // Only prefetch if we have a cache to populate
        if self.ctx.saved_cache.is_none() {
            trace!(
                target: "engine::tree::payload_processor::prewarm",
                "Skipping BAL prewarm - no cache available"
            );
            let _ =
                actions_tx.send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: 0 });
            return;
        }

        let total_slots = total_slots(&bal);

        trace!(
            target: "engine::tree::payload_processor::prewarm",
            total_slots,
            max_concurrency = self.max_concurrency,
            "Starting BAL prewarm"
        );

        if total_slots == 0 {
            // No slots to prefetch, signal completion immediately
            let _ =
                actions_tx.send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: 0 });
            return;
        }

        let (done_tx, done_rx) = mpsc::channel();

        // Calculate number of workers needed (at most max_concurrency)
        let workers_needed = total_slots.min(self.max_concurrency);

        // Calculate slots per worker
        let slots_per_worker = total_slots / workers_needed;
        let remainder = total_slots % workers_needed;

        // Spawn workers with their assigned ranges
        for i in 0..workers_needed {
            let start = i * slots_per_worker + i.min(remainder);
            let extra = if i < remainder { 1 } else { 0 };
            let end = start + slots_per_worker + extra;

            self.ctx.spawn_bal_worker(
                i,
                &self.executor,
                Arc::clone(&bal),
                start..end,
                done_tx.clone(),
            );
        }

        // Drop our handle to done_tx so we can detect completion
        drop(done_tx);

        // Wait for all workers to complete
        let mut completed_workers = 0;
        while done_rx.recv().is_ok() {
            completed_workers += 1;
        }

        trace!(
            target: "engine::tree::payload_processor::prewarm",
            completed_workers,
            "All BAL prewarm workers completed"
        );

        // Signal that execution has finished
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
    pub(super) fn run<Tx>(
        self,
        mode: PrewarmMode<Tx>,
        actions_tx: Sender<PrewarmTaskEvent<N::Receipt>>,
    ) where
        Tx: ExecutableTxFor<Evm> + Clone + Send + 'static,
    {
        // Spawn execution tasks based on mode
        match mode {
            PrewarmMode::Transactions(pending) => {
                self.spawn_all(pending, actions_tx);
            }
            PrewarmMode::BlockAccessList(bal) => {
                self.run_bal_prewarm(bal, actions_tx);
            }
        }

        let mut final_execution_outcome = None;
        let mut finished_execution = false;
        while let Ok(event) = self.actions_rx.recv() {
            match event {
                PrewarmTaskEvent::TerminateTransactionExecution => {
                    // stop tx processing
                    debug!(target: "engine::tree::prewarm", "Terminating prewarm execution");
                    self.ctx.terminate_execution.store(true, Ordering::Relaxed);
                }
                PrewarmTaskEvent::Outcome { proof_targets } => {
                    // completed executing a set of transactions
                    self.send_multi_proof_targets(proof_targets);
                }
                PrewarmTaskEvent::Terminate { execution_outcome } => {
                    trace!(target: "engine::tree::payload_processor::prewarm", "Received termination signal");
                    final_execution_outcome = Some(execution_outcome);

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
        if let Some(Some(execution_outcome)) = final_execution_outcome {
            self.save_cache(execution_outcome);
        }
    }
}

/// Context required by tx execution tasks.
#[derive(Debug, Clone)]
pub(super) struct PrewarmContext<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    pub(super) env: ExecutionEnv<Evm>,
    pub(super) evm_config: Evm,
    pub(super) saved_cache: Option<SavedCache>,
    /// Provider to obtain the state
    pub(super) provider: StateProviderBuilder<N, P>,
    pub(super) metrics: PrewarmMetrics,
    /// An atomic bool that tells prewarm tasks to not start any more execution.
    pub(super) terminate_execution: Arc<AtomicBool>,
    pub(super) precompile_cache_disabled: bool,
    pub(super) precompile_cache_map: PrecompileCacheMap<SpecFor<Evm>>,
}

impl<N, P, Evm> PrewarmContext<N, P, Evm>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    /// Splits this context into an evm, an evm config, metrics, and the atomic bool for terminating
    /// execution.
    #[instrument(level = "debug", target = "engine::tree::payload_processor::prewarm", skip_all)]
    fn evm_for_ctx(self) -> Option<(EvmFor<Evm, impl Database>, PrewarmMetrics, Arc<AtomicBool>)> {
        let Self {
            env,
            evm_config,
            saved_cache,
            provider,
            metrics,
            terminate_execution,
            precompile_cache_disabled,
            precompile_cache_map,
        } = self;

        let mut state_provider = match provider.build() {
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
        if let Some(saved_cache) = saved_cache {
            let caches = saved_cache.cache().clone();
            let cache_metrics = saved_cache.metrics().clone();
            state_provider = Box::new(
                CachedStateProvider::new(state_provider, caches, cache_metrics)
                    // ensure we pre-warm the cache
                    .prewarm(),
            );
        }

        let state_provider = StateProviderDatabase::new(state_provider);

        let mut evm_env = env.evm_env;

        // we must disable the nonce check so that we can execute the transaction even if the nonce
        // doesn't match what's on chain.
        evm_env.cfg_env.disable_nonce_check = true;

        // create a new executor and disable nonce checks in the env
        let spec_id = *evm_env.spec_id();
        let mut evm = evm_config.evm_with_env(state_provider, evm_env);

        if !precompile_cache_disabled {
            // Only cache pure precompiles to avoid issues with stateful precompiles
            evm.precompiles_mut().map_pure_precompiles(|address, precompile| {
                CachedPrecompile::wrap(
                    precompile,
                    precompile_cache_map.cache_for_address(*address),
                    spec_id,
                    None, // No metrics for prewarm
                )
            });
        }

        Some((evm, metrics, terminate_execution))
    }

    /// Accepts an [`mpsc::Receiver`] of transactions and a handle to prewarm task. Executes
    /// transactions and streams [`PrewarmTaskEvent::Outcome`] messages for each transaction.
    ///
    /// This function processes transactions sequentially from the receiver and emits outcome events
    /// via the provided sender. Execution errors are logged and tracked but do not stop the batch
    /// processing unless the task is explicitly cancelled.
    ///
    /// Note: There are no ordering guarantees; this does not reflect the state produced by
    /// sequential execution.
    #[instrument(level = "debug", target = "engine::tree::payload_processor::prewarm", skip_all)]
    fn transact_batch<Tx>(
        self,
        txs: mpsc::Receiver<IndexedTransaction<Tx>>,
        sender: Sender<PrewarmTaskEvent<N::Receipt>>,
        done_tx: Sender<()>,
    ) where
        Tx: ExecutableTxFor<Evm>,
    {
        let Some((mut evm, metrics, terminate_execution)) = self.evm_for_ctx() else { return };

        while let Ok(IndexedTransaction { index, tx }) = {
            let _enter = debug_span!(target: "engine::tree::payload_processor::prewarm", "recv tx")
                .entered();
            txs.recv()
        } {
            let enter =
                debug_span!(target: "engine::tree::payload_processor::prewarm", "prewarm tx", index, tx_hash=%tx.tx().tx_hash())
                    .entered();

            // create the tx env
            let start = Instant::now();

            // If the task was cancelled, stop execution, send an empty result to notify the task,
            // and exit.
            if terminate_execution.load(Ordering::Relaxed) {
                let _ = sender.send(PrewarmTaskEvent::Outcome { proof_targets: None });
                break
            }

            let res = match evm.transact(&tx) {
                Ok(res) => res,
                Err(err) => {
                    trace!(
                        target: "engine::tree::payload_processor::prewarm",
                        %err,
                        tx_hash=%tx.tx().tx_hash(),
                        sender=%tx.signer(),
                        "Error when executing prewarm transaction",
                    );
                    // Track transaction execution errors
                    metrics.transaction_errors.increment(1);
                    // skip error because we can ignore these errors and continue with the next tx
                    continue
                }
            };
            metrics.execution_duration.record(start.elapsed());

            // record some basic information about the transactions
            enter.record("gas_used", res.result.gas_used());
            enter.record("is_success", res.result.is_success());

            drop(enter);

            // If the task was cancelled, stop execution, send an empty result to notify the task,
            // and exit.
            if terminate_execution.load(Ordering::Relaxed) {
                let _ = sender.send(PrewarmTaskEvent::Outcome { proof_targets: None });
                break
            }

            // Only send outcome for transactions after the first txn
            // as the main execution will be just as fast
            if index > 0 {
                let _enter =
                    debug_span!(target: "engine::tree::payload_processor::prewarm", "prewarm outcome", index, tx_hash=%tx.tx().tx_hash())
                        .entered();
                let (targets, storage_targets) = multiproof_targets_from_state(res.state);
                metrics.prefetch_storage_targets.record(storage_targets as f64);
                let _ = sender.send(PrewarmTaskEvent::Outcome { proof_targets: Some(targets) });
                drop(_enter);
            }

            metrics.total_runtime.record(start.elapsed());
        }

        // send a message to the main task to flag that we're done
        let _ = done_tx.send(());
    }

    /// Spawns a worker task for transaction execution and returns its sender channel.
    fn spawn_workers<Tx>(
        self,
        workers_needed: usize,
        task_executor: &WorkloadExecutor,
        actions_tx: Sender<PrewarmTaskEvent<N::Receipt>>,
        done_tx: Sender<()>,
    ) -> Vec<mpsc::Sender<IndexedTransaction<Tx>>>
    where
        Tx: ExecutableTxFor<Evm> + Send + 'static,
    {
        let mut handles = Vec::with_capacity(workers_needed);
        let mut receivers = Vec::with_capacity(workers_needed);

        for _ in 0..workers_needed {
            let (tx, rx) = mpsc::channel();
            handles.push(tx);
            receivers.push(rx);
        }

        // Spawn a separate task spawning workers in parallel.
        let executor = task_executor.clone();
        let span = Span::current();
        task_executor.spawn_blocking(move || {
            let _enter = span.entered();
            for (idx, rx) in receivers.into_iter().enumerate() {
                let ctx = self.clone();
                let actions_tx = actions_tx.clone();
                let done_tx = done_tx.clone();
                let span = debug_span!(target: "engine::tree::payload_processor::prewarm", "prewarm worker", idx);
                executor.spawn_blocking(move || {
                    let _enter = span.entered();
                    ctx.transact_batch(rx, actions_tx, done_tx);
                });
            }
        });

        handles
    }

    /// Spawns a worker task for BAL slot prefetching.
    ///
    /// The worker iterates over the specified range of slots in the BAL and ensures
    /// each slot is loaded into the cache by accessing it through the state provider.
    fn spawn_bal_worker(
        &self,
        idx: usize,
        executor: &WorkloadExecutor,
        bal: Arc<BlockAccessList>,
        range: Range<usize>,
        done_tx: Sender<()>,
    ) {
        let ctx = self.clone();
        let span = debug_span!(
            target: "engine::tree::payload_processor::prewarm",
            "bal prewarm worker",
            idx,
            range_start = range.start,
            range_end = range.end
        );

        executor.spawn_blocking(move || {
            let _enter = span.entered();
            ctx.prefetch_bal_slots(bal, range, done_tx);
        });
    }

    /// Prefetches storage slots from a BAL range into the cache.
    ///
    /// This iterates through the specified range of slots and accesses them via the state
    /// provider to populate the cache.
    #[instrument(level = "debug", target = "engine::tree::payload_processor::prewarm", skip_all)]
    fn prefetch_bal_slots(
        self,
        bal: Arc<BlockAccessList>,
        range: Range<usize>,
        done_tx: Sender<()>,
    ) {
        let Self { saved_cache, provider, metrics, .. } = self;

        // Build state provider
        let state_provider = match provider.build() {
            Ok(provider) => provider,
            Err(err) => {
                trace!(
                    target: "engine::tree::payload_processor::prewarm",
                    %err,
                    "Failed to build state provider in BAL prewarm thread"
                );
                let _ = done_tx.send(());
                return;
            }
        };

        // Wrap with cache (guaranteed to be Some since run_bal_prewarm checks)
        let saved_cache = saved_cache.expect("BAL prewarm should only run with cache");
        let caches = saved_cache.cache().clone();
        let cache_metrics = saved_cache.metrics().clone();
        let state_provider = CachedStateProvider::new(state_provider, caches, cache_metrics);

        let start = Instant::now();

        // Track last seen address to avoid fetching the same account multiple times.
        let mut last_address = None;

        // Iterate through the assigned range of slots
        for (address, slot) in BALSlotIter::new(&bal, range.clone()) {
            // Fetch the account if this is a different address than the last one
            if last_address != Some(address) {
                let _ = state_provider.basic_account(&address);
                last_address = Some(address);
            }

            // Access the slot to populate the cache
            let _ = state_provider.storage(address, slot);
        }

        let elapsed = start.elapsed();

        trace!(
            target: "engine::tree::payload_processor::prewarm",
            ?range,
            elapsed_ms = elapsed.as_millis(),
            "BAL prewarm worker completed"
        );

        // Signal completion
        let _ = done_tx.send(());
        metrics.bal_slot_iteration_duration.record(elapsed.as_secs_f64());
    }
}

/// Returns a set of [`MultiProofTargets`] and the total amount of storage targets, based on the
/// given state.
fn multiproof_targets_from_state(state: EvmState) -> (MultiProofTargets, usize) {
    let mut targets = MultiProofTargets::with_capacity(state.len());
    let mut storage_targets = 0;
    for (addr, account) in state {
        // if the account was not touched, or if the account was selfdestructed, do not
        // fetch proofs for it
        //
        // Since selfdestruct can only happen in the same transaction, we can skip
        // prefetching proofs for selfdestructed accounts
        //
        // See: https://eips.ethereum.org/EIPS/eip-6780
        if !account.is_touched() || account.is_selfdestructed() {
            continue
        }

        let mut storage_set =
            B256Set::with_capacity_and_hasher(account.storage.len(), Default::default());
        for (key, slot) in account.storage {
            // do nothing if unchanged
            if !slot.is_changed() {
                continue
            }

            storage_set.insert(keccak256(B256::new(key.to_be_bytes())));
        }

        storage_targets += storage_set.len();
        targets.insert(keccak256(addr), storage_set);
    }

    (targets, storage_targets)
}

/// The events the pre-warm task can handle.
///
/// Generic over `R` (receipt type) to allow sharing `Arc<ExecutionOutcome<R>>` with the main
/// execution path without cloning the expensive `BundleState`.
pub(super) enum PrewarmTaskEvent<R> {
    /// Forcefully terminate all remaining transaction execution.
    TerminateTransactionExecution,
    /// Forcefully terminate the task on demand and update the shared cache with the given output
    /// before exiting.
    Terminate {
        /// The final execution outcome. Using `Arc` allows sharing with the main execution
        /// path without cloning the expensive `BundleState`.
        execution_outcome: Option<Arc<ExecutionOutcome<R>>>,
    },
    /// The outcome of a pre-warm task
    Outcome {
        /// The prepared proof targets based on the evm state outcome
        proof_targets: Option<MultiProofTargets>,
    },
    /// Finished executing all transactions
    FinishedTxExecution {
        /// Number of transactions executed
        executed_transactions: usize,
    },
}

/// Metrics for transactions prewarming.
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.prewarm")]
pub(crate) struct PrewarmMetrics {
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
