//! Entrypoint for payload processing.

use crate::tree::{
    cached_state::{CachedStateMetrics, ProviderCacheBuilder, ProviderCaches, SavedCache},
    payload_processor::{
        prewarm::{PrewarmCacheTask, PrewarmContext, PrewarmTaskEvent},
        sparse_trie::StateRootComputeOutcome,
    },
    sparse_trie::SparseTrieTask,
    StateProviderBuilder, TreeConfig,
};
use alloy_evm::{block::StateChangeSource, ToTxEnv};
use alloy_primitives::B256;
use executor::WorkloadExecutor;
use multiproof::{SparseTrieUpdate, *};
use parking_lot::RwLock;
use prewarm::PrewarmMetrics;
use reth_engine_primitives::ExecutableTxIterator;
use reth_evm::{
    execute::{ExecutableTxFor, WithTxEnv},
    ConfigureEvm, EvmEnvFor, OnStateHook, SpecFor, TxEnvFor,
};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DatabaseProviderFactory, StateCommitmentProvider,
    StateProviderFactory, StateReader,
};
use reth_revm::{db::BundleState, state::EvmState};
use reth_trie::TrieInput;
use reth_trie_parallel::{
    proof_task::{ProofTaskCtx, ProofTaskManager},
    root::ParallelStateRootError,
};
use reth_trie_sparse::{
    provider::{TrieNodeProvider, TrieNodeProviderFactory},
    ClearedSparseStateTrie, SerialSparseTrie, SparseStateTrie, SparseTrie,
};
use std::sync::{
    atomic::AtomicBool,
    mpsc::{self, channel, Sender},
    Arc,
};

use super::precompile_cache::PrecompileCacheMap;

mod configured_sparse_trie;
pub mod executor;
pub mod multiproof;
pub mod prewarm;
pub mod sparse_trie;

use configured_sparse_trie::ConfiguredSparseTrie;

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
    cross_block_cache_size: u64,
    /// Whether transactions should not be executed on prewarming task.
    disable_transaction_prewarming: bool,
    /// Determines how to configure the evm for execution.
    evm_config: Evm,
    /// Whether precompile cache should be disabled.
    precompile_cache_disabled: bool,
    /// Precompile cache map.
    precompile_cache_map: PrecompileCacheMap<SpecFor<Evm>>,
    /// A cleared `SparseStateTrie`, kept around to be reused for the state root computation so
    /// that allocations can be minimized.
    sparse_state_trie: Arc<
        parking_lot::Mutex<Option<ClearedSparseStateTrie<ConfiguredSparseTrie, SerialSparseTrie>>>,
    >,
    /// Whether to use the parallel sparse trie.
    use_parallel_sparse_trie: bool,
    /// A cleared trie input, kept around to be reused so allocations can be minimized.
    trie_input: Option<TrieInput>,
}

impl<N, Evm> PayloadProcessor<Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
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
            disable_transaction_prewarming: config.disable_caching_and_prewarming(),
            evm_config,
            precompile_cache_disabled: config.precompile_cache_disabled(),
            precompile_cache_map,
            sparse_state_trie: Arc::default(),
            trie_input: None,
            use_parallel_sparse_trie: config.enable_parallel_sparse_trie(),
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
    pub fn spawn<P, I: ExecutableTxIterator<Evm>>(
        &mut self,
        env: ExecutionEnv<Evm>,
        transactions: I,
        provider_builder: StateProviderBuilder<N, P>,
        consistent_view: ConsistentDbView<P>,
        trie_input: TrieInput,
        config: &TreeConfig,
    ) -> PayloadHandle<WithTxEnv<TxEnvFor<Evm>, I::Tx>, I::Error>
    where
        P: DatabaseProviderFactory<Provider: BlockReader>
            + BlockReader
            + StateProviderFactory
            + StateReader
            + StateCommitmentProvider
            + Clone
            + 'static,
    {
        let (to_sparse_trie, sparse_trie_rx) = channel();
        // spawn multiproof task, save the trie input
        let (trie_input, state_root_config) =
            MultiProofConfig::new_from_input(consistent_view, trie_input);
        self.trie_input = Some(trie_input);

        // Create and spawn the storage proof task
        let task_ctx = ProofTaskCtx::new(
            state_root_config.nodes_sorted.clone(),
            state_root_config.state_sorted.clone(),
            state_root_config.prefix_sets.clone(),
        );
        let max_proof_task_concurrency = config.max_proof_task_concurrency() as usize;
        let proof_task = ProofTaskManager::new(
            self.executor.handle().clone(),
            state_root_config.consistent_view.clone(),
            task_ctx,
            max_proof_task_concurrency,
        );

        // We set it to half of the proof task concurrency, because often for each multiproof we
        // spawn one Tokio task for the account proof, and one Tokio task for the storage proof.
        let max_multi_proof_task_concurrency = max_proof_task_concurrency / 2;
        let multi_proof_task = MultiProofTask::new(
            state_root_config,
            self.executor.clone(),
            proof_task.handle(),
            to_sparse_trie,
            max_multi_proof_task_concurrency,
        );

        // wire the multiproof task to the prewarm task
        let to_multi_proof = Some(multi_proof_task.state_root_message_sender());

        let (prewarm_rx, execution_rx) = self.spawn_tx_iterator(transactions);

        let prewarm_handle =
            self.spawn_caching_with(env, prewarm_rx, provider_builder, to_multi_proof.clone());

        // spawn multi-proof task
        self.executor.spawn_blocking(move || {
            multi_proof_task.run();
        });

        // wire the sparse trie to the state root response receiver
        let (state_root_tx, state_root_rx) = channel();

        // Spawn the sparse trie task using any stored trie and parallel trie configuration.
        self.spawn_sparse_trie_task(sparse_trie_rx, proof_task.handle(), state_root_tx);

        // spawn the proof task
        self.executor.spawn_blocking(move || {
            if let Err(err) = proof_task.run() {
                // At least log if there is an error at any point
                tracing::error!(
                    target: "engine::root",
                    ?err,
                    "Storage proof task returned an error"
                );
            }
        });

        PayloadHandle {
            to_multi_proof,
            prewarm_handle,
            state_root: Some(state_root_rx),
            transactions: execution_rx,
        }
    }

    /// Spawn cache prewarming exclusively.
    ///
    /// Returns a [`PayloadHandle`] to communicate with the task.
    pub(super) fn spawn_cache_exclusive<P, I: ExecutableTxIterator<Evm>>(
        &self,
        env: ExecutionEnv<Evm>,
        transactions: I,
        provider_builder: StateProviderBuilder<N, P>,
    ) -> PayloadHandle<WithTxEnv<TxEnvFor<Evm>, I::Tx>, I::Error>
    where
        P: BlockReader
            + StateProviderFactory
            + StateReader
            + StateCommitmentProvider
            + Clone
            + 'static,
    {
        let (prewarm_rx, execution_rx) = self.spawn_tx_iterator(transactions);
        let prewarm_handle = self.spawn_caching_with(env, prewarm_rx, provider_builder, None);
        PayloadHandle {
            to_multi_proof: None,
            prewarm_handle,
            state_root: None,
            transactions: execution_rx,
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
    ) {
        let (prewarm_tx, prewarm_rx) = mpsc::channel();
        let (execute_tx, execute_rx) = mpsc::channel();
        self.executor.spawn_blocking(move || {
            for tx in transactions {
                let tx = tx.map(|tx| WithTxEnv { tx_env: tx.to_tx_env(), tx });
                // only send Ok(_) variants to prewarming task
                if let Ok(tx) = &tx {
                    let _ = prewarm_tx.send(tx.clone());
                }
                let _ = execute_tx.send(tx);
            }
        });

        (prewarm_rx, execute_rx)
    }

    /// Spawn prewarming optionally wired to the multiproof task for target updates.
    fn spawn_caching_with<P>(
        &self,
        env: ExecutionEnv<Evm>,
        mut transactions: mpsc::Receiver<impl ExecutableTxFor<Evm> + Send + 'static>,
        provider_builder: StateProviderBuilder<N, P>,
        to_multi_proof: Option<Sender<MultiProofMessage>>,
    ) -> CacheTaskHandle
    where
        P: BlockReader
            + StateProviderFactory
            + StateReader
            + StateCommitmentProvider
            + Clone
            + 'static,
    {
        if self.disable_transaction_prewarming {
            // if no transactions should be executed we clear them but still spawn the task for
            // caching updates
            transactions = mpsc::channel().1;
        }

        let (cache, cache_metrics) = self.cache_for(env.parent_hash).split();
        // configure prewarming
        let prewarm_ctx = PrewarmContext {
            env,
            evm_config: self.evm_config.clone(),
            cache: cache.clone(),
            cache_metrics: cache_metrics.clone(),
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
        );

        // spawn pre-warm task
        {
            let to_prewarm_task = to_prewarm_task.clone();
            self.executor.spawn_blocking(move || {
                prewarm_task.run(transactions, to_prewarm_task);
            });
        }

        CacheTaskHandle { cache, to_prewarm_task: Some(to_prewarm_task), cache_metrics }
    }

    /// Takes the trie input from the inner payload processor, if it exists.
    pub const fn take_trie_input(&mut self) -> Option<TrieInput> {
        self.trie_input.take()
    }

    /// Returns the cache for the given parent hash.
    ///
    /// If the given hash is different then what is recently cached, then this will create a new
    /// instance.
    fn cache_for(&self, parent_hash: B256) -> SavedCache {
        self.execution_cache.get_cache_for(parent_hash).unwrap_or_else(|| {
            let cache = ProviderCacheBuilder::default().build_caches(self.cross_block_cache_size);
            SavedCache::new(parent_hash, cache, CachedStateMetrics::zeroed())
        })
    }

    /// Spawns the [`SparseTrieTask`] for this payload processor.
    fn spawn_sparse_trie_task<BPF>(
        &self,
        sparse_trie_rx: mpsc::Receiver<SparseTrieUpdate>,
        proof_task_handle: BPF,
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
            let accounts_trie = if self.use_parallel_sparse_trie {
                ConfiguredSparseTrie::Parallel(Default::default())
            } else {
                ConfiguredSparseTrie::Serial(Default::default())
            };
            ClearedSparseStateTrie::from_state_trie(
                SparseStateTrie::new()
                    .with_accounts_trie(SparseTrie::Blind(Some(Box::new(accounts_trie))))
                    .with_updates(true),
            )
        });

        let task =
            SparseTrieTask::<_, ConfiguredSparseTrie, SerialSparseTrie>::new_with_cleared_trie(
                self.executor.clone(),
                sparse_trie_rx,
                proof_task_handle,
                self.trie_metrics.clone(),
                sparse_state_trie,
            );

        self.executor.spawn_blocking(move || {
            let (result, trie) = task.run();
            // Send state root computation result
            let _ = state_root_tx.send(result);

            // Clear the SparseStateTrie and replace it back into the mutex _after_ sending results
            // to the next step, so that time spent clearing doesn't block the step after this one.
            cleared_sparse_trie.lock().replace(ClearedSparseStateTrie::from_state_trie(trie));
        });
    }
}

/// Handle to all the spawned tasks.
#[derive(Debug)]
pub struct PayloadHandle<Tx, Err> {
    /// Channel for evm state updates
    to_multi_proof: Option<Sender<MultiProofMessage>>,
    // must include the receiver of the state root wired to the sparse trie
    prewarm_handle: CacheTaskHandle,
    /// Receiver for the state root
    state_root: Option<mpsc::Receiver<Result<StateRootComputeOutcome, ParallelStateRootError>>>,
    /// Stream of block transactions
    transactions: mpsc::Receiver<Result<Tx, Err>>,
}

impl<Tx, Err> PayloadHandle<Tx, Err> {
    /// Awaits the state root
    ///
    /// # Panics
    ///
    /// If payload processing was started without background tasks.
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
                let _ = sender.send(MultiProofMessage::StateUpdate(source, state.clone()));
            }
        }
    }

    /// Returns a clone of the caches used by prewarming
    pub(super) fn caches(&self) -> ProviderCaches {
        self.prewarm_handle.cache.clone()
    }

    pub(super) fn cache_metrics(&self) -> CachedStateMetrics {
        self.prewarm_handle.cache_metrics.clone()
    }

    /// Terminates the pre-warming transaction processing.
    ///
    /// Note: This does not terminate the task yet.
    pub(super) fn stop_prewarming_execution(&self) {
        self.prewarm_handle.stop_prewarming_execution()
    }

    /// Terminates the entire caching task.
    ///
    /// If the [`BundleState`] is provided it will update the shared cache.
    pub(super) fn terminate_caching(&mut self, block_output: Option<BundleState>) {
        self.prewarm_handle.terminate_caching(block_output)
    }

    /// Returns iterator yielding transactions from the stream.
    pub fn iter_transactions(&mut self) -> impl Iterator<Item = Result<Tx, Err>> + '_ {
        core::iter::repeat_with(|| self.transactions.recv())
            .take_while(|res| res.is_ok())
            .map(|res| res.unwrap())
    }
}

/// Access to the spawned [`PrewarmCacheTask`].
#[derive(Debug)]
pub(crate) struct CacheTaskHandle {
    /// The shared cache the task operates with.
    cache: ProviderCaches,
    /// Metrics for the caches
    cache_metrics: CachedStateMetrics,
    /// Channel to the spawned prewarm task if any
    to_prewarm_task: Option<Sender<PrewarmTaskEvent>>,
}

impl CacheTaskHandle {
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
    /// If the [`BundleState`] is provided it will update the shared cache.
    pub(super) fn terminate_caching(&mut self, block_output: Option<BundleState>) {
        self.to_prewarm_task
            .take()
            .map(|tx| tx.send(PrewarmTaskEvent::Terminate { block_output }).ok());
    }
}

impl Drop for CacheTaskHandle {
    fn drop(&mut self) {
        // Ensure we always terminate on drop
        self.terminate_caching(None);
    }
}

/// Shared access to most recently used cache.
///
/// This cache is intended to used for processing the payload in the following manner:
///  - Get Cache if the payload's parent block matches the parent block
///  - Update cache upon successful payload execution
///
/// This process assumes that payloads are received sequentially.
#[derive(Clone, Debug, Default)]
struct ExecutionCache {
    /// Guarded cloneable cache identified by a block hash.
    inner: Arc<RwLock<Option<SavedCache>>>,
}

impl ExecutionCache {
    /// Returns the cache if the currently store cache is for the given `parent_hash`
    pub(crate) fn get_cache_for(&self, parent_hash: B256) -> Option<SavedCache> {
        let cache = self.inner.read();
        cache
            .as_ref()
            .and_then(|cache| (cache.executed_block_hash() == parent_hash).then(|| cache.clone()))
    }

    /// Clears the tracked cache
    #[expect(unused)]
    pub(crate) fn clear(&self) {
        self.inner.write().take();
    }

    /// Stores the provider cache
    pub(crate) fn save_cache(&self, cache: SavedCache) {
        self.inner.write().replace(cache);
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
    use crate::tree::{
        payload_processor::{
            evm_state_to_hashed_post_state, executor::WorkloadExecutor, PayloadProcessor,
        },
        precompile_cache::PrecompileCacheMap,
        StateProviderBuilder, TreeConfig,
    };
    use alloy_evm::block::StateChangeSource;
    use rand::Rng;
    use reth_chainspec::ChainSpec;
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::TransactionSigned;
    use reth_evm::OnStateHook;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::{Account, Recovered, StorageEntry};
    use reth_provider::{
        providers::{BlockchainProvider, ConsistentDbView},
        test_utils::create_test_provider_factory_with_chain_spec,
        ChainSpecProvider, HashingWriter,
    };
    use reth_testing_utils::generators;
    use reth_trie::{test_utils::state_root, HashedPostState, TrieInput};
    use revm_primitives::{Address, HashMap, B256, KECCAK_EMPTY, U256};
    use revm_state::{AccountInfo, AccountStatus, EvmState, EvmStorageSlot};
    use std::sync::Arc;

    fn create_mock_state_updates(num_accounts: usize, updates_per_account: usize) -> Vec<EvmState> {
        let mut rng = generators::rng();
        let all_addresses: Vec<Address> = (0..num_accounts).map(|_| rng.random()).collect();
        let mut updates = Vec::new();

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
        let provider = BlockchainProvider::new(factory).unwrap();
        let mut handle = payload_processor.spawn(
            Default::default(),
            core::iter::empty::<Result<Recovered<TransactionSigned>, core::convert::Infallible>>(),
            StateProviderBuilder::new(provider.clone(), genesis_hash, None),
            ConsistentDbView::new_with_latest_tip(provider).unwrap(),
            TrieInput::from_state(hashed_state),
            &TreeConfig::default(),
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
