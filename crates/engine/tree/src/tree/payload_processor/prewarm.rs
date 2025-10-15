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
        executor::WorkloadExecutor, multiproof::MultiProofMessage,
        ExecutionCache as PayloadExecutionCache,
    },
    precompile_cache::{CachedPrecompile, PrecompileCacheMap},
    ExecutionEnv, StateProviderBuilder,
};
use alloy_consensus::transaction::TxHashRef;
use alloy_eips::Typed2718;
use alloy_evm::Database;
use alloy_primitives::{keccak256, map::B256Set, B256};
use metrics::{Counter, Gauge, Histogram};
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm, Evm, EvmFor, SpecFor};
use reth_metrics::Metrics;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{BlockReader, StateProviderFactory, StateReader};
use reth_revm::{database::StateProviderDatabase, db::BundleState, state::EvmState};
use reth_trie::MultiProofTargets;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, channel, Receiver, Sender},
        Arc,
    },
    time::Instant,
};
use tracing::{debug, trace, warn};

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
    to_multi_proof: Option<Sender<MultiProofMessage>>,
    /// Receiver for events produced by tx execution
    actions_rx: Receiver<PrewarmTaskEvent>,
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
        to_multi_proof: Option<Sender<MultiProofMessage>>,
        transaction_count_hint: usize,
        max_concurrency: usize,
    ) -> (Self, Sender<PrewarmTaskEvent>) {
        let (actions_tx, actions_rx) = channel();

        trace!(
            target: "engine::tree::prewarm",
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
            },
            actions_tx,
        )
    }

    /// Spawns all pending transactions as blocking tasks by first chunking them.
    ///
    /// For Optimism chains, special handling is applied to the first transaction if it's a
    /// deposit transaction (type 0x7E/126) which sets critical metadata that affects all
    /// subsequent transactions in the block.
    fn spawn_all<Tx>(&self, pending: mpsc::Receiver<Tx>, actions_tx: Sender<PrewarmTaskEvent>)
    where
        Tx: ExecutableTxFor<Evm> + Clone + Send + 'static,
    {
        let executor = self.executor.clone();
        let ctx = self.ctx.clone();
        let max_concurrency = self.max_concurrency;
        let transaction_count_hint = self.transaction_count_hint;

        self.executor.spawn_blocking(move || {
            let (done_tx, done_rx) = mpsc::channel();
            let mut executing = 0usize;

            // Initialize worker handles container
            let mut handles = Vec::with_capacity(max_concurrency);

            // When transaction_count_hint is 0, it means the count is unknown. In this case, spawn
            // max workers to handle potentially many transactions in parallel rather
            // than bottlenecking on a single worker.
            let workers_needed = if transaction_count_hint == 0 {
                max_concurrency
            } else {
                transaction_count_hint.min(max_concurrency)
            };

            // Only spawn initial workers as needed
            for _ in 0..workers_needed {
                handles.push(ctx.spawn_worker(&executor, actions_tx.clone(), done_tx.clone()));
            }

            let mut tx_index = 0usize;

            // Handle first transaction - special case for system transactions
            if let Ok(first_tx) = pending.recv() {
                // Move the transaction into the indexed wrapper to avoid an extra clone
                let indexed_tx = IndexedTransaction { index: tx_index, tx: first_tx };
                // Compute metadata from the moved value
                let tx_ref = indexed_tx.tx.tx();
                let is_system_tx = tx_ref.ty() > MAX_STANDARD_TX_TYPE;
                let first_tx_hash = tx_ref.tx_hash();

                // Check if this is a system transaction (type > 4)
                // System transactions in the first position typically set critical metadata
                // that affects all subsequent transactions (e.g., L1 block info, fees on L2s).
                if is_system_tx {
                    // Broadcast system transaction to all workers to ensure they have the
                    // critical state. This is particularly important for L2s like Optimism
                    // where the first deposit transaction contains essential block metadata.
                    for handle in &handles {
                        if let Err(err) = handle.send(indexed_tx.clone()) {
                            warn!(
                                target: "engine::tree::prewarm",
                                tx_hash = %first_tx_hash,
                                error = %err,
                                "Failed to send deposit transaction to worker"
                            );
                        }
                    }
                } else {
                    // Not a deposit, send to first worker via round-robin
                    if let Err(err) = handles[0].send(indexed_tx) {
                        warn!(
                            target: "engine::tree::prewarm",
                            task_idx = 0,
                            error = %err,
                            "Failed to send transaction to worker"
                        );
                    }
                }
                executing += 1;
                tx_index += 1;
            }

            // Process remaining transactions with round-robin distribution
            while let Ok(executable) = pending.recv() {
                let indexed_tx = IndexedTransaction { index: tx_index, tx: executable };
                let task_idx = executing % workers_needed;
                if let Err(err) = handles[task_idx].send(indexed_tx) {
                    warn!(
                        target: "engine::tree::prewarm",
                        task_idx,
                        error = %err,
                        "Failed to send transaction to worker"
                    );
                }
                executing += 1;
                tx_index += 1;
            }

            // drop handle and wait for all tasks to finish and drop theirs
            drop(done_tx);
            drop(handles);
            while done_rx.recv().is_ok() {}

            let _ = actions_tx
                .send(PrewarmTaskEvent::FinishedTxExecution { executed_transactions: executing });
        });
    }

    /// If configured and the tx returned proof targets, emit the targets the transaction produced
    fn send_multi_proof_targets(&self, targets: Option<MultiProofTargets>) {
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
    fn save_cache(self, state: BundleState) {
        let start = Instant::now();

        let Self { execution_cache, ctx: PrewarmContext { env, metrics, saved_cache, .. }, .. } =
            self;
        let hash = env.hash;

        // Perform all cache operations atomically under the lock
        execution_cache.update_with_guard(|cached| {

            // consumes the `SavedCache` held by the prewarming task, which releases its usage guard
            let (caches, cache_metrics) = saved_cache.split();
            let new_cache = SavedCache::new(hash, caches, cache_metrics);

            // Insert state into cache while holding the lock
            if new_cache.cache().insert_state(&state).is_err() {
                // Clear the cache on error to prevent having a polluted cache
                *cached = None;
                debug!(target: "engine::caching", "cleared execution cache on update error");
                return;
            }

            new_cache.update_metrics();
            debug!(target: "engine::caching", parent_hash=?new_cache.executed_block_hash(), "Updated execution cache");

            // Replace the shared cache with the new one; the previous cache (if any) is dropped.
            *cached = Some(new_cache);
        });

        metrics.cache_saving_duration.set(start.elapsed().as_secs_f64());
    }

    /// Executes the task.
    ///
    /// This will execute the transactions until all transactions have been processed or the task
    /// was cancelled.
    pub(super) fn run(
        self,
        pending: mpsc::Receiver<impl ExecutableTxFor<Evm> + Clone + Send + 'static>,
        actions_tx: Sender<PrewarmTaskEvent>,
    ) {
        // spawn execution tasks.
        self.spawn_all(pending, actions_tx);

        let mut final_block_output = None;
        let mut finished_execution = false;
        while let Ok(event) = self.actions_rx.recv() {
            match event {
                PrewarmTaskEvent::TerminateTransactionExecution => {
                    // stop tx processing
                    self.ctx.terminate_execution.store(true, Ordering::Relaxed);
                }
                PrewarmTaskEvent::Outcome { proof_targets } => {
                    // completed executing a set of transactions
                    self.send_multi_proof_targets(proof_targets);
                }
                PrewarmTaskEvent::Terminate { block_output } => {
                    trace!(target: "engine::tree::prewarm", "Received termination signal");
                    final_block_output = Some(block_output);

                    if finished_execution {
                        // all tasks are done, we can exit, which will save caches and exit
                        break
                    }
                }
                PrewarmTaskEvent::FinishedTxExecution { executed_transactions } => {
                    trace!(target: "engine::tree::prewarm", "Finished prewarm execution signal");
                    self.ctx.metrics.transactions.set(executed_transactions as f64);
                    self.ctx.metrics.transactions_histogram.record(executed_transactions as f64);

                    finished_execution = true;

                    if final_block_output.is_some() {
                        // all tasks are done, we can exit, which will save caches and exit
                        break
                    }
                }
            }
        }

        trace!(target: "engine::tree::prewarm", "Completed prewarm execution");

        // save caches and finish
        if let Some(Some(state)) = final_block_output {
            self.save_cache(state);
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
    pub(super) saved_cache: SavedCache,
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
    fn evm_for_ctx(self) -> Option<(EvmFor<Evm, impl Database>, PrewarmMetrics, Arc<AtomicBool>)> {
        let Self {
            env,
            evm_config,
            saved_cache,
            provider,
            metrics,
            terminate_execution,
            precompile_cache_disabled,
            mut precompile_cache_map,
        } = self;

        let state_provider = match provider.build() {
            Ok(provider) => provider,
            Err(err) => {
                trace!(
                    target: "engine::tree",
                    %err,
                    "Failed to build state provider in prewarm thread"
                );
                return None
            }
        };

        // Use the caches to create a new provider with caching
        let caches = saved_cache.cache().clone();
        let cache_metrics = saved_cache.metrics().clone();
        let state_provider =
            CachedStateProvider::new_with_caches(state_provider, caches, cache_metrics);

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
    /// Returns `None` if executing the transactions failed to a non Revert error.
    /// Returns the touched+modified state of the transaction.
    ///
    /// Note: There are no ordering guarantees; this does not reflect the state produced by
    /// sequential execution.
    fn transact_batch<Tx>(
        self,
        txs: mpsc::Receiver<IndexedTransaction<Tx>>,
        sender: Sender<PrewarmTaskEvent>,
        done_tx: Sender<()>,
    ) where
        Tx: ExecutableTxFor<Evm>,
    {
        let Some((mut evm, metrics, terminate_execution)) = self.evm_for_ctx() else { return };

        while let Ok(IndexedTransaction { index, tx }) = txs.recv() {
            // If the task was cancelled, stop execution, send an empty result to notify the task,
            // and exit.
            if terminate_execution.load(Ordering::Relaxed) {
                let _ = sender.send(PrewarmTaskEvent::Outcome { proof_targets: None });
                break
            }

            // create the tx env
            let start = Instant::now();
            let res = match evm.transact(&tx) {
                Ok(res) => res,
                Err(err) => {
                    trace!(
                        target: "engine::tree::prewarm",
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

            // Only send outcome for transactions after the first txn
            // as the main execution will be just as fast
            if index > 0 {
                let (targets, storage_targets) = multiproof_targets_from_state(res.state);
                metrics.prefetch_storage_targets.record(storage_targets as f64);
                let _ = sender.send(PrewarmTaskEvent::Outcome { proof_targets: Some(targets) });
            }

            metrics.total_runtime.record(start.elapsed());
        }

        // send a message to the main task to flag that we're done
        let _ = done_tx.send(());
    }

    /// Spawns a worker task for transaction execution and returns its sender channel.
    fn spawn_worker<Tx>(
        &self,
        executor: &WorkloadExecutor,
        actions_tx: Sender<PrewarmTaskEvent>,
        done_tx: Sender<()>,
    ) -> mpsc::Sender<IndexedTransaction<Tx>>
    where
        Tx: ExecutableTxFor<Evm> + Clone + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let ctx = self.clone();

        executor.spawn_blocking(move || {
            ctx.transact_batch(rx, actions_tx, done_tx);
        });

        tx
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
pub(super) enum PrewarmTaskEvent {
    /// Forcefully terminate all remaining transaction execution.
    TerminateTransactionExecution,
    /// Forcefully terminate the task on demand and update the shared cache with the given output
    /// before exiting.
    Terminate {
        /// The final block state output.
        block_output: Option<BundleState>,
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
}
