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
    payload_processor::{bal, multiproof::MultiProofMessage, PayloadExecutionCache},
    precompile_cache::{CachedPrecompile, PrecompileCacheMap},
    ExecutionEnv, StateProviderBuilder,
};
use alloy_consensus::transaction::TxHashRef;
use alloy_eip7928::BlockAccessList;
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{keccak256, StorageKey, B256};
use crossbeam_channel::Sender as CrossbeamSender;
use metrics::{Counter, Gauge, Histogram};
use rayon::prelude::*;
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm, Evm, EvmFor, RecoveredTx, SpecFor};
use reth_metrics::Metrics;
use reth_primitives_traits::{FastInstant as Instant, NodePrimitives};
use reth_provider::{
    AccountReader, BlockExecutionOutput, BlockReader, StateProvider, StateProviderFactory,
    StateReader,
};
use reth_revm::{database::StateProviderDatabase, state::EvmState};
use reth_tasks::{pool::WorkerPool, Runtime};
use reth_trie_common::{MultiProofTargetsV2, ProofV2Target};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::{self, channel, Receiver, Sender},
    Arc,
};
use tracing::{debug, debug_span, instrument, trace, warn, Span};

/// Determines the prewarming mode: transaction-based, BAL-based, or skipped.
#[derive(Debug)]
pub enum PrewarmMode<Tx> {
    /// Prewarm by executing transactions from a stream, each paired with its block index.
    Transactions(Receiver<(usize, Tx)>),
    /// Prewarm by prefetching slots from a Block Access List.
    BlockAccessList(Arc<BlockAccessList>),
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
    pub fn new(
        executor: Runtime,
        execution_cache: PayloadExecutionCache,
        ctx: PrewarmContext<N, P, Evm>,
        to_multi_proof: Option<CrossbeamSender<MultiProofMessage>>,
    ) -> (Self, Sender<PrewarmTaskEvent<N::Receipt>>) {
        let (actions_tx, actions_rx) = channel();

        trace!(
            target: "engine::tree::payload_processor::prewarm",
            prewarming_threads = executor.prewarming_pool().current_num_threads(),
            transaction_count = ctx.env.transaction_count,
            "Initialized prewarm task"
        );

        (
            Self {
                executor,
                execution_cache,
                ctx,
                to_multi_proof,
                actions_rx,
                parent_span: Span::current(),
            },
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
        to_multi_proof: Option<CrossbeamSender<MultiProofMessage>>,
    ) where
        Tx: ExecutableTxFor<Evm> + Send + 'static,
    {
        let executor = self.executor.clone();
        let ctx = self.ctx.clone();
        let span = Span::current();

        self.executor.spawn_blocking_named("prewarm-txs", move || {
            let _enter = debug_span!(
                target: "engine::tree::payload_processor::prewarm",
                parent: span,
                "prewarm_txs"
            )
            .entered();

            let ctx = &ctx;
            let pool = executor.prewarming_pool();

            let mut tx_count = 0usize;
            let to_multi_proof = to_multi_proof.as_ref();
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
                        let _enter = debug_span!(
                            target: "engine::tree::payload_processor::prewarm",
                            parent: parent_span,
                            "prewarm_tx",
                            i = index,
                        )
                        .entered();
                        Self::transact_worker(ctx, index, tx, to_multi_proof);
                    });
                }

                // Send withdrawal prefetch targets after all transactions dispatched
                if let Some(to_multi_proof) = to_multi_proof &&
                    let Some(withdrawals) = &ctx.env.withdrawals &&
                    !withdrawals.is_empty()
                {
                    let targets = multiproof_targets_from_withdrawals(withdrawals);
                    let _ = to_multi_proof.send(MultiProofMessage::PrefetchProofs(targets));
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
        to_multi_proof: Option<&CrossbeamSender<MultiProofMessage>>,
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
                let (targets, storage_targets) = multiproof_targets_from_state(res.state);
                ctx.metrics.prefetch_storage_targets.record(storage_targets as f64);
                if let Some(to_multi_proof) = to_multi_proof {
                    let _ = to_multi_proof.send(MultiProofMessage::PrefetchProofs(targets));
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
    /// This consumes the `SavedCache` held by the task, which releases its usage guard and allows
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

        let Self { execution_cache, ctx: PrewarmContext { env, metrics, saved_cache, .. }, .. } =
            self;
        let hash = env.hash;

        if let Some(saved_cache) = saved_cache {
            debug!(target: "engine::caching", parent_hash=?hash, "Updating execution cache");
            // Perform all cache operations atomically under the lock
            execution_cache.update_with_guard(|cached| {
                // consumes the `SavedCache` held by the prewarming task, which releases its usage
                // guard
                let (caches, cache_metrics, disable_cache_metrics) = saved_cache.split();
                let new_cache = SavedCache::new(hash, caches, cache_metrics)
                    .with_disable_cache_metrics(disable_cache_metrics);

                // Insert state into cache while holding the lock
                // Access the BundleState through the shared ExecutionOutcome
                if new_cache.cache().insert_state(&execution_outcome.state).is_err() {
                    // Clear the cache on error to prevent having a polluted cache
                    *cached = None;
                    debug!(target: "engine::caching", "cleared execution cache on update error");
                    return;
                }

                new_cache.update_metrics();

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

    /// Runs BAL-based prewarming by using the prewarming pool's parallel iterator to prefetch
    /// accounts and storage slots.
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
            self.send_bal_hashed_state(&bal);
            let _ =
                actions_tx.send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: 0 });
            return;
        }

        if bal.is_empty() {
            self.send_bal_hashed_state(&bal);
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
        self.executor.prewarming_pool().install_fn(|| {
            bal.par_iter().for_each_init(
                || (ctx.clone(), None::<CachedStateProvider<reth_provider::StateProviderBox>>),
                |(ctx, provider), account| {
                    if ctx.should_stop() {
                        return;
                    }
                    ctx.prefetch_bal_account(provider, account);
                },
            );
        });

        trace!(
            target: "engine::tree::payload_processor::prewarm",
            "All BAL prewarm accounts completed"
        );

        // Convert BAL to HashedPostState and send to multiproof task
        self.send_bal_hashed_state(&bal);

        // Signal that execution has finished
        let _ = actions_tx.send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: 0 });
    }

    /// Converts the BAL to [`HashedPostState`](reth_trie::HashedPostState) and sends it to the
    /// multiproof task.
    fn send_bal_hashed_state(&self, bal: &BlockAccessList) {
        let Some(to_multi_proof) = &self.to_multi_proof else { return };

        let provider = match self.ctx.provider.build() {
            Ok(provider) => provider,
            Err(err) => {
                warn!(
                    target: "engine::tree::payload_processor::prewarm",
                    ?err,
                    "Failed to build provider for BAL hashed state conversion"
                );
                return;
            }
        };

        match bal::bal_to_hashed_post_state(bal, &provider) {
            Ok(hashed_state) => {
                debug!(
                    target: "engine::tree::payload_processor::prewarm",
                    accounts = hashed_state.accounts.len(),
                    storages = hashed_state.storages.len(),
                    "Converted BAL to hashed post state"
                );
                let _ = to_multi_proof.send(MultiProofMessage::HashedStateUpdate(hashed_state));
                let _ = to_multi_proof.send(MultiProofMessage::FinishedStateUpdates);
            }
            Err(err) => {
                warn!(
                    target: "engine::tree::payload_processor::prewarm",
                    ?err,
                    "Failed to convert BAL to hashed state"
                );
            }
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
        // Spawn execution tasks based on mode
        match mode {
            PrewarmMode::Transactions(pending) => {
                self.spawn_txs_prewarm(pending, actions_tx, self.to_multi_proof.clone());
            }
            PrewarmMode::BlockAccessList(bal) => {
                self.run_bal_prewarm(bal, actions_tx);
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
    pub provider: StateProviderBuilder<N, P>,
    /// The metrics for the prewarm task.
    pub metrics: PrewarmMetrics,
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
}

/// Per-thread EVM state initialised by [`PrewarmContext::evm_for_ctx`] and stored in
/// [`WorkerPool`] workers via [`Worker::get_or_init`](reth_tasks::pool::Worker::get_or_init).
type PrewarmEvmState<Evm> =
    Option<EvmFor<Evm, StateProviderDatabase<reth_provider::StateProviderBox>>>;

impl<N, P, Evm> PrewarmContext<N, P, Evm>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + 'static,
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
            let cache_metrics = saved_cache.metrics().clone();
            state_provider =
                Box::new(CachedStateProvider::new_prewarm(state_provider, caches, cache_metrics));
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

    /// Prefetches a single account and all its storage slots from the BAL into the cache.
    ///
    /// The `provider` is lazily initialized on first call and reused across accounts on the same
    /// thread.
    fn prefetch_bal_account(
        &self,
        provider: &mut Option<CachedStateProvider<reth_provider::StateProviderBox>>,
        account: &alloy_eip7928::AccountChanges,
    ) {
        let state_provider = match provider {
            Some(p) => p,
            slot @ None => {
                let built = match self.provider.build() {
                    Ok(p) => p,
                    Err(err) => {
                        trace!(
                            target: "engine::tree::payload_processor::prewarm",
                            %err,
                            "Failed to build state provider in BAL prewarm thread"
                        );
                        return;
                    }
                };
                let saved_cache =
                    self.saved_cache.as_ref().expect("BAL prewarm should only run with cache");
                let caches = saved_cache.cache().clone();
                let cache_metrics = saved_cache.metrics().clone();
                slot.insert(CachedStateProvider::new(built, caches, cache_metrics))
            }
        };

        let start = Instant::now();

        let _ = state_provider.basic_account(&account.address);

        for slot in &account.storage_changes {
            let _ = state_provider.storage(account.address, StorageKey::from(slot.slot));
        }
        for &slot in &account.storage_reads {
            let _ = state_provider.storage(account.address, StorageKey::from(slot));
        }

        self.metrics.bal_slot_iteration_duration.record(start.elapsed().as_secs_f64());
    }
}

/// Returns a set of [`MultiProofTargetsV2`] and the total amount of storage targets, based on the
/// given state.
fn multiproof_targets_from_state(state: EvmState) -> (MultiProofTargetsV2, usize) {
    let mut targets = MultiProofTargetsV2::default();
    let mut storage_target_count = 0;
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

        let hashed_address = keccak256(addr);
        targets.account_targets.push(hashed_address.into());

        let mut storage_slots = Vec::with_capacity(account.storage.len());
        for (key, slot) in account.storage {
            // do nothing if unchanged
            if !slot.is_changed() {
                continue
            }

            let hashed_slot = keccak256(B256::new(key.to_be_bytes()));
            storage_slots.push(ProofV2Target::from(hashed_slot));
        }

        storage_target_count += storage_slots.len();
        if !storage_slots.is_empty() {
            targets.storage_targets.insert(hashed_address, storage_slots);
        }
    }

    (targets, storage_target_count)
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

/// The events the pre-warm task can handle.
///
/// Generic over `R` (receipt type) to allow sharing `Arc<ExecutionOutcome<R>>` with the main
/// execution path without cloning the expensive `BundleState`.
#[derive(Debug)]
pub enum PrewarmTaskEvent<R> {
    /// Forcefully terminate all remaining transaction execution.
    TerminateTransactionExecution,
    /// Forcefully terminate the task on demand and update the shared cache with the given output
    /// before exiting.
    Terminate {
        /// The final execution outcome. Using `Arc` allows sharing with the main execution
        /// path without cloning the expensive `BundleState`.
        execution_outcome: Option<Arc<BlockExecutionOutput<R>>>,
        /// Receiver for the block validation result.
        ///
        /// Cache saving is racing the state root validation. We optimistically construct the
        /// updated cache but only save it once we know the block is valid.
        valid_block_rx: mpsc::Receiver<()>,
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
