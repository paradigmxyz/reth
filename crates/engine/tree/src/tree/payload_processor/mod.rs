//! Entrypoint for payload processing.

use crate::tree::{
    cached_state::{CachedStateMetrics, ProviderCacheBuilder, ProviderCaches, SavedCache},
    payload_processor::{
        executor::WorkloadExecutor,
        prewarm::{PrewarmCacheTask, PrewarmContext, PrewarmTaskEvent},
        sparse_trie::{SparseTrieTask, StateRootComputeOutcome},
    },
    StateProviderBuilder, TreeConfig,
};
use alloy_consensus::{transaction::Recovered, BlockHeader};
use alloy_primitives::B256;
use multiproof::*;
use parking_lot::RwLock;
use reth_evm::{
    system_calls::{OnStateHook, StateChangeSource},
    ConfigureEvm, ConfigureEvmEnvFor,
};
use reth_primitives_traits::{NodePrimitives, SealedHeaderFor};
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DatabaseProviderFactory, StateCommitmentProvider,
    StateProviderFactory, StateReader,
};
use reth_revm::{db::BundleState, state::EvmState};
use reth_trie::TrieInput;
use reth_trie_parallel::root::ParallelStateRootError;
use std::{
    collections::VecDeque,
    sync::{
        mpsc,
        mpsc::{channel, Sender},
        Arc,
    },
};

pub(crate) mod executor;

mod multiproof;
mod prewarm;
mod sparse_trie;

/// Entrypoint for executing the payload.
pub(super) struct PayloadProcessor<N, Evm> {
    /// The executor used by to spawn tasks.
    executor: WorkloadExecutor,
    /// The most recent cache used for execution.
    execution_cache: ExecutionCache,
    /// Metrics for trie operations
    trie_metrics: MultiProofTaskMetrics,
    /// Cross-block cache size in bytes.
    cross_block_cache_size: u64,
    /// Whether transactions should be executed on prewarming task.
    use_transaction_prewarming: bool,
    /// Determines how to configure the evm for execution.
    evm_config: Evm,
    _marker: std::marker::PhantomData<N>,
}

impl<N, Evm> PayloadProcessor<N, Evm> {
    pub(super) fn new(executor: WorkloadExecutor, evm_config: Evm, config: &TreeConfig) -> Self {
        Self {
            executor,
            execution_cache: Default::default(),
            trie_metrics: Default::default(),
            cross_block_cache_size: config.cross_block_cache_size(),
            use_transaction_prewarming: config.use_caching_and_prewarming(),
            evm_config,
            _marker: Default::default(),
        }
    }
}

impl<N, Evm> PayloadProcessor<N, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvmEnvFor<N>
        + 'static
        + ConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>
        + 'static,
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
    /// This task runs until it receives a shutdown signal, which should be after after the block
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
    pub(super) fn spawn<P>(
        &self,
        header: SealedHeaderFor<N>,
        transactions: VecDeque<Recovered<N::SignedTx>>,
        provider_builder: StateProviderBuilder<N, P>,
        consistent_view: ConsistentDbView<P>,
        trie_input: TrieInput,
    ) -> PayloadHandle
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
        // spawn multiproof task
        let state_root_config = MultiProofConfig::new_from_input(consistent_view, trie_input);
        let multi_proof_task =
            MultiProofTask::new(state_root_config.clone(), self.executor.clone(), to_sparse_trie);

        // wire the multiproof task to the prewarm task
        let to_multi_proof = Some(multi_proof_task.state_root_message_sender());

        let prewarm_handle =
            self.spawn_caching_with(header, transactions, provider_builder, to_multi_proof.clone());

        // spawn multi-proof task
        self.executor.spawn_blocking(move || {
            multi_proof_task.run();
        });

        let sparse_trie_task = SparseTrieTask {
            executor: self.executor.clone(),
            updates: sparse_trie_rx,
            config: state_root_config,
            metrics: self.trie_metrics.clone(),
        };

        // wire the sparse trie to the state root response receiver
        let (state_root_tx, state_root_rx) = channel();
        self.executor.spawn_blocking(move || {
            let res = sparse_trie_task.run();
            let _ = state_root_tx.send(res);
        });

        PayloadHandle { to_multi_proof, prewarm_handle, state_root: Some(state_root_rx) }
    }

    /// Spawn cache prewarming exclusively.
    ///
    /// Returns a [`PayloadHandle`] to communicate with the task.
    pub(super) fn spawn_cache_exclusive<P>(
        &self,
        header: SealedHeaderFor<N>,
        transactions: VecDeque<Recovered<N::SignedTx>>,
        provider_builder: StateProviderBuilder<N, P>,
    ) -> PayloadHandle
    where
        P: BlockReader
            + StateProviderFactory
            + StateReader
            + StateCommitmentProvider
            + Clone
            + 'static,
    {
        let prewarm_handle = self.spawn_caching_with(header, transactions, provider_builder, None);
        PayloadHandle { to_multi_proof: None, prewarm_handle, state_root: None }
    }

    /// Spawn prewarming optionally wired to the multiproof task for target updates.
    fn spawn_caching_with<P>(
        &self,
        header: SealedHeaderFor<N>,
        mut transactions: VecDeque<Recovered<N::SignedTx>>,
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
        if !self.use_transaction_prewarming {
            // if no transactions should be executed we clear them but still spawn the task for
            // caching updates
            transactions.clear();
        }

        let (cache, cache_metrics) = self.cache_for(header.parent_hash()).split();
        // configure prewarming
        let prewarm_ctx = PrewarmContext {
            header,
            evm_config: self.evm_config.clone(),
            cache: cache.clone(),
            cache_metrics: cache_metrics.clone(),
            provider: provider_builder,
        };

        let prewarm_task = PrewarmCacheTask::new(
            self.executor.clone(),
            self.execution_cache.clone(),
            prewarm_ctx,
            to_multi_proof,
            transactions,
        );
        let to_prewarm_task = prewarm_task.actions_tx();

        // spawn pre-warm task
        self.executor.spawn_blocking(move || {
            prewarm_task.run();
        });
        CacheTaskHandle { cache, to_prewarm_task: Some(to_prewarm_task), cache_metrics }
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
}

/// Handle to all the spawned tasks.
pub(super) struct PayloadHandle {
    /// Channel for evm state updates
    to_multi_proof: Option<Sender<MultiProofMessage>>,
    // must include the receiver of the state root wired to the sparse trie
    prewarm_handle: CacheTaskHandle,
    /// Receiver for the state root
    state_root: Option<mpsc::Receiver<Result<StateRootComputeOutcome, ParallelStateRootError>>>,
}

impl PayloadHandle {
    /// Awaits the state root
    ///
    /// # Panics
    ///
    /// If payload processing was started without background tasks.
    pub(super) fn state_root(&mut self) -> Result<StateRootComputeOutcome, ParallelStateRootError> {
        self.state_root
            .take()
            .expect("state_root is None")
            .recv()
            .map_err(|_| ParallelStateRootError::Other("sparse trie task dropped".to_string()))?
    }

    /// Returns a state hook to be used to send state updates to this task.
    ///
    /// If a multiproof task is spawned the hook will notify it about new states.
    pub(super) fn state_hook(&self) -> impl OnStateHook {
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
}

/// Access to the spawned [`PrewarmCacheTask`].
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

    /// Clears the tracked cashe
    #[allow(unused)]
    pub(crate) fn clear(&self) {
        self.inner.write().take();
    }

    /// Stores the provider cache
    pub(crate) fn save_cache(&self, cache: SavedCache) {
        self.inner.write().replace(cache);
    }
}
