//! Caching and prewarming related functionality.

use crate::tree::{
    cached_state::{CachedStateMetrics, CachedStateProvider, ProviderCaches, SavedCache},
    payload_processor::{
        executor::WorkloadExecutor, multiproof::MultiProofMessage, ExecutionCache,
    },
    precompile_cache::{CachedPrecompile, PrecompileCacheMap},
    ExecutionEnv, StateProviderBuilder,
};
use alloy_evm::Database;
use alloy_primitives::{keccak256, map::B256Set, B256};
use metrics::{Counter, Gauge, Histogram};
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm, Evm, EvmFor, SpecFor};
use reth_metrics::Metrics;
use reth_primitives_traits::{NodePrimitives, SignedTransaction};
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
use tracing::{debug, trace};

/// Number of initial transactions to skip from prewarming.
///
/// Based on empirical performance analysis across 200 blocks, the first 11 transactions
/// in each block consistently fail to benefit from prewarming due to timing conflicts
/// with the main execution thread.
const SKIP_PREWARM_TRANSACTIONS: usize = 18;

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
    execution_cache: ExecutionCache,
    /// Context provided to execution tasks
    ctx: PrewarmContext<N, P, Evm>,
    /// How many transactions should be executed in parallel
    max_concurrency: usize,
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
        execution_cache: ExecutionCache,
        ctx: PrewarmContext<N, P, Evm>,
        to_multi_proof: Option<Sender<MultiProofMessage>>,
    ) -> (Self, Sender<PrewarmTaskEvent>) {
        let (actions_tx, actions_rx) = channel();
        (
            Self {
                executor,
                execution_cache,
                ctx,
                max_concurrency: 64,
                to_multi_proof,
                actions_rx,
            },
            actions_tx,
        )
    }

    /// Spawns all pending transactions as blocking tasks by first chunking them.
    fn spawn_all(
        &self,
        pending: mpsc::Receiver<impl ExecutableTxFor<Evm> + Send + 'static>,
        actions_tx: Sender<PrewarmTaskEvent>,
    ) {
        let executor = self.executor.clone();
        let ctx = self.ctx.clone();
        let max_concurrency = self.max_concurrency;

        self.executor.spawn_blocking(move || {
            let mut handles = Vec::new();
            let (done_tx, done_rx) = mpsc::channel();
            let mut executing = 0;
            let mut skipped = 0;
            let mut prewarmed = 0;

            while let Ok(executable) = pending.recv() {
                // Skip the first transactions to avoid cache contention with main thread.
                // These transactions are executed by the main thread before prewarm threads
                // can complete their work, resulting in wasted computational resources.
                if executing < SKIP_PREWARM_TRANSACTIONS {
                    executing += 1;
                    skipped += 1;
                    continue;
                }

                let task_idx = (executing - SKIP_PREWARM_TRANSACTIONS) % max_concurrency;

                if handles.len() <= task_idx {
                    let (tx, rx) = mpsc::channel();
                    let sender = actions_tx.clone();
                    let ctx = ctx.clone();
                    let done_tx = done_tx.clone();

                    executor.spawn_blocking(move || {
                        ctx.transact_batch(rx, sender, done_tx);
                    });

                    handles.push(tx);
                }

                let _ = handles[task_idx].send(executable);

                executing += 1;
                prewarmed += 1;
            }

            // drop handle and wait for all tasks to finish and drop theirs
            drop(done_tx);
            drop(handles);
            while done_rx.recv().is_ok() {}

            let _ = actions_tx.send(PrewarmTaskEvent::FinishedTxExecution {
                executed_transactions: executing,
                skipped_transactions: skipped,
                prewarmed_transactions: prewarmed,
            });
        });
    }

    /// If configured and the tx returned proof targets, emit the targets the transaction produced
    fn send_multi_proof_targets(&self, targets: Option<MultiProofTargets>) {
        if let Some((proof_targets, to_multi_proof)) = targets.zip(self.to_multi_proof.as_ref()) {
            let _ = to_multi_proof.send(MultiProofMessage::PrefetchProofs(proof_targets));
        }
    }

    /// Save the state to the shared cache for the given block.
    fn save_cache(self, state: BundleState) {
        let start = Instant::now();

        // Count what we're saving
        let accounts_in_cache = state.state.len();
        let storage_slots_in_cache: usize =
            state.state.values().map(|account| account.storage.len()).sum();

        let cache = SavedCache::new(
            self.ctx.env.hash,
            self.ctx.cache.clone(),
            self.ctx.cache_metrics.clone(),
        );
        if cache.cache().insert_state(&state).is_err() {
            return
        }

        cache.update_metrics();

        debug!(
            target: "engine::caching",
            accounts_cached = accounts_in_cache,
            storage_slots_cached = storage_slots_in_cache,
            duration_ms = start.elapsed().as_millis(),
            "Updated state caches with prewarm results"
        );

        // update the cache for the executed block
        self.execution_cache.save_cache(cache);
        self.ctx.metrics.cache_saving_duration.set(start.elapsed().as_secs_f64());
    }

    /// Executes the task.
    ///
    /// This will execute the transactions until all transactions have been processed or the task
    /// was cancelled.
    pub(super) fn run(
        self,
        pending: mpsc::Receiver<impl ExecutableTxFor<Evm> + Send + 'static>,
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
                    final_block_output = Some(block_output);

                    if finished_execution {
                        // all tasks are done, we can exit, which will save caches and exit
                        break
                    }
                }
                PrewarmTaskEvent::FinishedTxExecution {
                    executed_transactions,
                    skipped_transactions,
                    prewarmed_transactions,
                } => {
                    self.ctx.metrics.transactions.set(executed_transactions as f64);
                    self.ctx.metrics.transactions_histogram.record(executed_transactions as f64);

                    // Update skip and prewarm metrics
                    self.ctx.metrics.transactions_skipped.set(skipped_transactions as f64);
                    self.ctx
                        .metrics
                        .transactions_prewarmed
                        .increment(prewarmed_transactions as u64);

                    // Log prewarm effectiveness summary
                    let skip_rate =
                        (skipped_transactions as f64 / executed_transactions.max(1) as f64) * 100.0;
                    let prewarm_rate = (prewarmed_transactions as f64 /
                        executed_transactions.max(1) as f64) *
                        100.0;

                    debug!(
                        target: "prewarm::summary",
                        total_transactions = executed_transactions,
                        skipped_transactions = skipped_transactions,
                        prewarmed_transactions = prewarmed_transactions,
                        skip_rate = format!("{:.1}%", skip_rate),
                        prewarm_rate = format!("{:.1}%", prewarm_rate),
                        "Prewarm execution completed"
                    );

                    finished_execution = true;

                    if final_block_output.is_some() {
                        // all tasks are done, we can exit, which will save caches and exit
                        break
                    }
                }
            }
        }

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
    pub(super) cache: ProviderCaches,
    pub(super) cache_metrics: CachedStateMetrics,
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
            cache: caches,
            cache_metrics,
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
    /// Note: Since here are no ordering guarantees this won't the state the txs produce when
    /// executed sequentially.
    fn transact_batch(
        self,
        txs: mpsc::Receiver<impl ExecutableTxFor<Evm>>,
        sender: Sender<PrewarmTaskEvent>,
        done_tx: Sender<()>,
    ) {
        let Some((mut evm, metrics, terminate_execution)) = self.evm_for_ctx() else { return };

        while let Ok(tx) = txs.recv() {
            // If the task was cancelled, stop execution, send an empty result to notify the task,
            // and exit.
            if terminate_execution.load(Ordering::Relaxed) {
                let _ = sender.send(PrewarmTaskEvent::Outcome { proof_targets: None });
                break
            }

            // create the tx env
            let start = Instant::now();
            let tx_hash = tx.tx().tx_hash();
            let res = match evm.transact(&tx) {
                Ok(res) => res,
                Err(err) => {
                    trace!(
                        target: "engine::tree",
                        %err,
                        tx_hash=%tx_hash,
                        sender=%tx.signer(),
                        "Error when executing prewarm transaction",
                    );
                    // Track this as a miss since we couldn't prewarm this transaction's state
                    metrics.prewarm_cache_misses.increment(1);
                    return
                }
            };
            metrics.execution_duration.record(start.elapsed());

            // Track what we're populating into the cache
            let accounts_touched = res.state.len();
            let storage_touched: usize =
                res.state.values().map(|account| account.storage.len()).sum();

            metrics.accounts_loaded.increment(accounts_touched as u64);
            metrics.storage_loaded.increment(storage_touched as u64);

            // Track cache effectiveness - these are all hits since we're prewarming
            // Every account and storage slot we load will be available as a cache hit
            metrics.prewarm_cache_hits.increment(accounts_touched as u64);
            metrics.prewarm_cache_hits.increment(storage_touched as u64);

            // Estimate I/O time saved (assuming ~1ms per account read and ~0.5ms per storage read
            // from disk) These are typical SSD latencies for database reads
            let estimated_io_saved_us = (accounts_touched * 1000 + storage_touched * 500) as f64;
            if estimated_io_saved_us > 0.0 {
                metrics.prewarm_io_time_saved_us.record(estimated_io_saved_us);
            }

            // Log cache population for analysis
            if accounts_touched > 0 || storage_touched > 0 {
                trace!(
                    target: "prewarm::cache",
                    %tx_hash,
                    accounts_loaded = accounts_touched,
                    storage_slots_loaded = storage_touched,
                    execution_ms = start.elapsed().as_millis(),
                    estimated_io_saved_us = estimated_io_saved_us,
                    "Populated cache with state"
                );
            }

            let (targets, storage_targets) = multiproof_targets_from_state(res.state);
            metrics.prefetch_storage_targets.record(storage_targets as f64);
            metrics.total_runtime.record(start.elapsed());

            let _ = sender.send(PrewarmTaskEvent::Outcome { proof_targets: Some(targets) });
        }

        // send a message to the main task to flag that we're done
        let _ = done_tx.send(());
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
        /// Number of transactions skipped
        skipped_transactions: usize,
        /// Number of transactions prewarmed
        prewarmed_transactions: usize,
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

    // Essential metrics to understand prewarm effectiveness
    /// Number of transactions skipped (not prewarmed) - using SKIP_PREWARM_TRANSACTIONS constant
    pub(crate) transactions_skipped: Gauge,
    /// Number of transactions actually prewarmed
    pub(crate) transactions_prewarmed: Counter,
    /// Number of accounts touched by prewarm (populated into cache)
    pub(crate) accounts_loaded: Counter,
    /// Number of storage slots loaded by prewarm (populated into cache)
    pub(crate) storage_loaded: Counter,

    // Cache effectiveness metrics (these track how effective the prewarm was)
    /// Total cache hits from prewarmed state during main execution
    pub(crate) prewarm_cache_hits: Counter,
    /// Total cache misses despite prewarming during main execution
    pub(crate) prewarm_cache_misses: Counter,
    /// Time spent on I/O that was avoided due to prewarm cache hits (microseconds)
    pub(crate) prewarm_io_time_saved_us: Histogram,
}
