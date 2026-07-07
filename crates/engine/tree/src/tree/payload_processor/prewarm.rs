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

use super::bal_prewarm_pool::BalPrewarmPool;
use crate::tree::{
    payload_processor::multiproof::{
        StateRootHashedUpdateStream, StateRootHintStream, StateRootStreams,
    },
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
    AccountReader, BlockExecutionOutput, BlockReader, StateProviderFactory, StateReader,
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
#[derive(Debug)]
pub enum PrewarmMode<Tx> {
    /// Prewarm by executing transactions from a stream, each paired with its block index.
    Transactions(Receiver<(usize, Tx)>),
    /// Prewarm by prefetching slots from a Block Access List.
    BlockAccessList(Arc<DecodedBal>),
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
    /// State-root streams used for prewarm hints and BAL-derived authoritative updates.
    state_root_streams: StateRootStreams,
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
        state_root_streams: StateRootStreams,
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
                state_root_streams,
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
    ) {
        let bal = decoded_bal.as_bal();
        if bal.is_empty() {
            if let Some(hashed_update_stream) = self.state_root_streams.hashed_update_stream() {
                hashed_update_stream.on_updates_finished();
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
        let hashed_update_stream = self.state_root_streams.hashed_update_stream();
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

                hashed_update_stream.on_updates_finished();
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
            let provider_builder = ctx.provider.clone();
            let build = Arc::new(move || provider_builder.build());

            pool.begin_block(build, caches);
            for account in prefetch_bal.as_bal() {
                pool.warm_account(account.address);

                let storage_slot_count =
                    account.storage_changes.len() + account.storage_reads.len();
                if storage_slot_count == 0 {
                    continue;
                }

                let mut storage_slots = Vec::with_capacity(storage_slot_count);
                for change in &account.storage_changes {
                    storage_slots.push(change.slot.into());
                }
                for &slot in &account.storage_reads {
                    storage_slots.push(slot.into());
                }
                pool.warm_storage_slots(account.address, storage_slots);
            }
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
                self.spawn_txs_prewarm(pending, actions_tx, self.state_root_streams.hint_stream());
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
            state_provider = Box::new(CachedStateProvider::new_prewarm(state_provider, caches));
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
        hashed_update_stream: &StateRootHashedUpdateStream,
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

        if !account_changes.storage_changes.is_empty() {
            let hashed_address = *hashed_address.get_or_insert_with(|| keccak256(address));
            let mut storage_map = reth_trie::HashedStorage::new(false);

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

                let inner = match self.provider.build() {
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
                            Box::new(CachedStateProvider::new_prewarm(inner, caches))
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
        let mut hashed_state = reth_trie::HashedPostState::default();
        hashed_state.accounts.insert(hashed_address, Some(account));

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
    use alloy_eip7928::{
        AccountChanges, BalanceChange, BlockAccessIndex, CodeChange, NonceChange, SlotChanges,
        StorageChange,
    };
    use alloy_primitives::{address, bytes};

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
