//! Caching and prewarming related functionality.

use crate::tree::{
    cached_state::{CachedStateMetrics, CachedStateProvider, ProviderCaches, SavedCache},
    payload_processor::{
        executor::WorkloadExecutor, multiproof::MultiProofMessage, ExecutionCache,
    },
    precompile_cache::{CachedPrecompile, PrecompileCacheMap},
    ExecutionEnv, StateProviderBuilder,
};
use alloy_consensus::Transaction;
use alloy_evm::Database;
use alloy_primitives::{keccak256, map::B256Set, B256};
use metrics::{Gauge, Histogram};
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm, Evm, EvmFor, SpecFor};
use reth_metrics::Metrics;
use reth_primitives_traits::{NodePrimitives, SignedTransaction};
use reth_provider::{BlockReader, StateProviderFactory, StateReader};
use reth_revm::{
    database::StateProviderDatabase,
    db::BundleState,
    state::{AccountInfo, Bytecode, EvmState},
};
use reth_trie::MultiProofTargets;
use revm::{
    bytecode::opcode,
    context::ContextTr,
    inspector::JournalExt,
    interpreter::{interpreter_types::Jumps, Interpreter},
    Inspector,
};
use revm_primitives::{Address, U256};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, channel, Receiver, Sender},
        Arc,
    },
    time::Instant,
};
use tracing::{debug, trace};

use super::TxCache;

/// Classifies a transaction based on its characteristics
fn classify_transaction<T, E>(tx: &T) -> &'static str 
where
    T: ExecutableTxFor<E>,
    E: ConfigureEvm
{
    let tx_inner = tx.tx();
    
    // Check if it's a contract creation (to address is None)
    if tx_inner.to().is_none() {
        return "contract_creation"
    }
    
    // Check if it's a simple transfer (no input data)
    if tx_inner.input().is_empty() || tx_inner.input().len() == 0 {
        return "simple_transfer"
    }
    
    // Check for common ERC-20 token transfer pattern
    // ERC-20 transfer method selector is 0xa9059cbb (first 4 bytes)
    if tx_inner.input().len() >= 4 {
        let selector = &tx_inner.input()[..4];
        
        // Common token operations
        if selector == [0xa9, 0x05, 0x9c, 0xbb] {  // transfer
            return "token_transfer"
        }
        if selector == [0x23, 0xb8, 0x72, 0xdd] {  // transferFrom
            return "token_transfer"
        }
        if selector == [0x09, 0x5e, 0xa7, 0xb3] {  // approve
            return "token_approval"
        }
        
        // Common DEX operations (Uniswap, etc)
        // swapExactTokensForTokens: 0x38ed1739
        // swapExactETHForTokens: 0x7ff36ab5
        // swapTokensForExactETH: 0x4a25d94a
        if selector == [0x38, 0xed, 0x17, 0x39] || 
           selector == [0x7f, 0xf3, 0x6a, 0xb5] ||
           selector == [0x4a, 0x25, 0xd9, 0x4a] {
            return "dex_swap"
        }
    }
    
    // Default to other for any contract interaction
    "other"
}

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
    tx_cache: TxCache<Evm>,
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
        tx_cache: TxCache<Evm>,
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
                tx_cache,
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

        let tx_cache = self.tx_cache.clone();
        
        // Track thread spawn time
        let spawn_start = Instant::now();
        
        self.executor.spawn_blocking(move || {
            let spawn_time = spawn_start.elapsed();
            metrics::histogram!("prewarm.thread_spawn_ms").record(spawn_time.as_millis() as f64);
            metrics::gauge!("prewarm.worker_count").set(max_concurrency as f64);
            let mut handles = Vec::new();
            let (done_tx, done_rx) = mpsc::channel();
            let mut executing = 0;
            while let Ok(executable) = pending.recv() {
                let task_idx = executing % max_concurrency;

                if handles.len() <= task_idx {
                    let (tx, rx) = mpsc::channel();
                    let sender = actions_tx.clone();
                    let ctx = ctx.clone();
                    let done_tx = done_tx.clone();

                    let tx_cache = tx_cache.clone();
                    executor.spawn_blocking(move || {
                        ctx.transact_batch(rx, tx_cache, sender, done_tx);
                    });

                    handles.push(tx);
                }

                let _ = handles[task_idx].send(executable);

                executing += 1;
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

    /// Save the state to the shared cache for the given block.
    fn save_cache(self, state: BundleState) {
        let start = Instant::now();
        let cache = SavedCache::new(
            self.ctx.env.hash,
            self.ctx.cache.clone(),
            self.ctx.cache_metrics.clone(),
        );
        if cache.cache().insert_state(&state).is_err() {
            return
        }

        cache.update_metrics();

        debug!(target: "engine::caching", "Updated state caches");

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
    fn evm_for_ctx(
        self,
    ) -> Option<(
        EvmFor<Evm, EVMRecordingDatabase<impl Database>, CoinbaseBalanceEVMInspector>,
        PrewarmMetrics,
        Arc<AtomicBool>,
    )> {
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
        let state_provider = EVMRecordingDatabase::new(state_provider);

        let mut evm_env = env.evm_env;
        let coinbase_address = evm_env.block_env().beneficiary;

        // we must disable the nonce check so that we can execute the transaction even if the nonce
        // doesn't match what's on chain.
        evm_env.cfg_env.disable_nonce_check = true;

        // create a new executor and disable nonce checks in the env
        let spec_id = *evm_env.spec_id();
        let mut evm = evm_config.evm_with_env_and_inspector(
            state_provider,
            evm_env,
            CoinbaseBalanceEVMInspector::new(coinbase_address),
        );

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
        tx_cache: TxCache<Evm>,
        sender: Sender<PrewarmTaskEvent>,
        done_tx: Sender<()>,
    ) {
        let Some((mut evm, metrics, terminate_execution)) = self.evm_for_ctx() else { return };
        let coinbase = evm.block().beneficiary;

        while let Ok(tx) = txs.recv() {
            let tx_hash = *tx.tx().tx_hash();
            
            // Classify transaction type
            let tx_type = classify_transaction::<_, Evm>(&tx);
            metrics::counter!("tx.type", "type" => tx_type).increment(1);

            // If the task was cancelled, stop execution, send an empty result to notify the task,
            // and exit.
            if terminate_execution.load(Ordering::Relaxed) {
                let _ = sender.send(PrewarmTaskEvent::Outcome { proof_targets: None });
                break
            }

            let coinbase_before = revm::Database::basic(evm.db_mut(), coinbase)
                .unwrap_or_default() // This should be erroring
                .unwrap_or_default();

            // create the tx env
            let start = Instant::now();
            let res = match evm.transact(&tx) {
                Ok(res) => res,
                Err(err) => {
                    trace!(
                        target: "engine::tree",
                        %err,
                        %tx_hash,
                        sender = %tx.signer(),
                        "Error when executing prewarm transaction",
                    );
                    return
                }
            };
            let execution_time = start.elapsed();
            metrics.execution_duration.record(execution_time);
            
            // Track execution time in prewarming metrics
            metrics::histogram!("prewarm.execution_ms").record(execution_time.as_millis() as f64);
            
            // Add transaction execution time metrics
            metrics::histogram!("tx.execution_time_us").record(execution_time.as_micros() as f64);
            
            // Calculate gas per second if gas was used
            let gas_used = res.result.gas_used();
            if gas_used > 0 && !execution_time.is_zero() {
                let gas_per_second = gas_used as f64 / execution_time.as_secs_f64();
                metrics::histogram!("tx.gas_per_second").record(gas_per_second);
            }

            let (targets, storage_targets) = multiproof_targets_from_state(&res.state);
            metrics.prefetch_storage_targets.record(storage_targets as f64);
            metrics.total_runtime.record(start.elapsed());

            let _ = sender.send(PrewarmTaskEvent::Outcome { proof_targets: Some(targets) });

            // Because coinbase is updated in every transaction with gas fees, we handle it in the
            // following way: if the only update is balance change from gas fees, we can re-use the
            // execution result and manually increase the balance by the delta; if the coinbase
            // balance was read, we cannot re-use the execution result because the balance may be
            // not correct as it doesn't have all previous increments from fees

            // Skip caching if coinbase balance was read, as the value would be incorrect without
            // all prior fee increments from previous transactions
            let coinbase_balance_read = std::mem::take(&mut evm.inspector_mut().balance_read);
            if coinbase_balance_read {
                tracing::debug!(
                    target: "engine::cache",
                    ?tx_hash,
                    "Cannot cache execution result - transaction reads coinbase balance"
                );
                continue
            }

            let execution_trace: Vec<AccessRecord> =
                std::mem::take(&mut evm.db_mut().recorded_traces);

            // Calculate coinbase nonce and balance deltas to reuse execution result by manually
            // adjusting for gas fee changes, if no read occurred and the only update is from fees
            let coinbase_deltas = res.state.get(&coinbase).map(|coinbase_after| {
                let nonce_delta = coinbase_after.info.nonce - coinbase_before.nonce;
                let balance_delta = coinbase_after.info.balance - coinbase_before.balance;
                tracing::trace!(
                    target: "engine::cache",
                    ?tx_hash,
                    ?coinbase_before,
                    ?coinbase_after,
                    ?nonce_delta,
                    ?balance_delta,
                    "Calculated coinbase deltas for cache"
                );
                (coinbase, nonce_delta, balance_delta)
            });

            // Deduplicate traces before caching
            let original_trace_count = execution_trace.len();
            let execution_trace = deduplicate_traces(execution_trace);
            let dedup_trace_count = execution_trace.len();
            
            tracing::debug!(
                target: "engine::cache",
                ?tx_hash,
                original_trace_count,
                dedup_trace_count,
                dedup_savings = original_trace_count.saturating_sub(dedup_trace_count),
                accounts_accessed = execution_trace.iter().filter(|t| matches!(t, AccessRecord::Account { .. })).count(),
                storage_accessed = execution_trace.iter().filter(|t| matches!(t, AccessRecord::Storage { .. })).count(),
                has_coinbase_deltas = coinbase_deltas.is_some(),
                gas_used = res.result.gas_used(),
                "Caching prewarmed execution result with deduplication"
            );

            // Create cached transaction with Arc-wrapped data
            let cached_tx = super::CachedTransaction {
                traces: execution_trace.into_boxed_slice().into(),
                result: Arc::new(res),
                coinbase_deltas,
            };
            tx_cache.insert(tx_hash, cached_tx);
        }

        // send a message to the main task to flag that we're done
        let _ = done_tx.send(());
    }
}

/// Returns a set of [`MultiProofTargets`] and the total amount of storage targets, based on the
/// given state.
fn multiproof_targets_from_state(state: &EvmState) -> (MultiProofTargets, usize) {
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
        for (key, slot) in &account.storage {
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

/// Captures state reads during transaction prewarming for cache validation.
/// Used to validate and reuse prewarmed results.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AccessRecord {
    /// Account access record containing account address and retrieved account information
    Account {
        /// The address of the account being accessed
        address: Address,
        /// The retrieved account information, if the account exists
        result: Option<AccountInfo>,
    },
    /// Storage slot access record containing contract address, storage key, and value
    Storage {
        /// The address of the contract whose storage is being accessed
        address: Address,
        /// The storage slot index being accessed
        index: U256,
        /// The value stored at the specified storage slot
        result: U256,
    },
}

/// Deduplicates access records while preserving the order of first occurrence.
/// This significantly reduces validation overhead for transactions with loops or repeated checks.
/// 
/// Returns a deduplicated vector of access records.
fn deduplicate_traces(traces: Vec<AccessRecord>) -> Vec<AccessRecord> {
    let mut seen = HashSet::new();
    let mut unique = Vec::with_capacity(traces.len().min(32)); // Cap initial capacity
    
    for record in traces {
        let key = match &record {
            AccessRecord::Account { address, .. } => (*address, None),
            AccessRecord::Storage { address, index, .. } => (*address, Some(*index)),
        };
        
        if seen.insert(key) {
            unique.push(record); // Keep first occurrence
        }
    }
    
    unique.shrink_to_fit();
    unique
}

/// revm database wrapper that records state access to validate state diffs
#[derive(Debug)]
struct EVMRecordingDatabase<DB> {
    inner_db: DB,
    recorded_traces: Vec<AccessRecord>,
}

impl<DB> EVMRecordingDatabase<DB> {
    fn new(inner_db: DB) -> Self {
        // Pre-allocate capacity for 32 traces (P95 of production workload).
        // Prevents 3-5 reallocations for 95% of transactions.
        // Only 5% need one reallocation (32→64). Saves ~2μs per transaction.
        Self { inner_db, recorded_traces: Vec::with_capacity(32) }
    }
}

impl<DB: revm::Database> revm::Database for EVMRecordingDatabase<DB> {
    type Error = DB::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let result = self.inner_db.basic(address)?;
        self.recorded_traces.push(AccessRecord::Account {
            address,
            result: result.as_ref().map(|r| r.copy_without_code()),
        });
        Ok(result)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.inner_db.code_by_hash(code_hash)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let result = self.inner_db.storage(address, index)?;
        self.recorded_traces.push(AccessRecord::Storage { address, index, result });
        Ok(result)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.inner_db.block_hash(number)
    }
}

/// Flags when a transaction reads the block coinbase's balance during EVM execution.
///
/// Rationale: During prewarming the coinbase balance is incomplete (prior tx fee credits
/// are not yet applied). If a tx observes that balance, its behavior may differ from a
/// real execution, so cached results must be disabled for safety.
#[derive(Debug)]
struct CoinbaseBalanceEVMInspector {
    coinbase: Address,
    balance_read: bool,
}

impl CoinbaseBalanceEVMInspector {
    /// Constructs an inspector for the given `coinbase` address. The `balance_read` flag
    /// starts as `false` and flips to `true` the first time a coinbase balance read is seen.
    const fn new(coinbase: Address) -> Self {
        Self { coinbase, balance_read: false }
    }
}

/// Tracks whether the coinbase address balance is read during EVM execution.
impl<CTX> Inspector<CTX> for CoinbaseBalanceEVMInspector
where
    CTX: ContextTr<Journal: JournalExt>,
{
    fn step(&mut self, interpreter: &mut Interpreter, _context: &mut CTX) {
        // Fast-path: once we've observed a read, avoid further work.
        if self.balance_read {
            return
        }

        match interpreter.bytecode.opcode() {
            opcode::BALANCE => {
                // BALANCE <addr>: mark if the queried address equals coinbase.
                if let Ok(addr) = interpreter.stack.peek(0) {
                    if Address::from_word(B256::from(addr.to_be_bytes())) == self.coinbase {
                        self.balance_read = true;
                    }
                }
            }
            opcode::SELFBALANCE => {
                // SELFBALANCE: mark if the current call target is the coinbase account.
                if interpreter.input.target_address == self.coinbase {
                    self.balance_read = true;
                }
            }
            _ => (),
        }
    }
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
}
