//! Entrypoint for payload processing.
#![allow(dead_code)] // TODO remove

use crate::tree::{
    cached_state::{CachedStateMetrics, ProviderCacheBuilder, ProviderCaches, SavedCache},
    payload_processor::{
        executor::WorkloadExecutor,
        prewarm::{PrewarmContext, PrewarmTask, PrewarmTaskEvent},
        sparse_trie::SparseTrieTask,
    },
    StateProviderBuilder,
};
use alloy_consensus::{transaction::Recovered, BlockHeader};
use alloy_primitives::B256;
use multiproof::*;
use parking_lot::RwLock;
use reth_evm::{
    execute::BlockExecutorProvider, system_calls::OnStateHook, ConfigureEvm, ConfigureEvmEnvFor,
    Evm,
};
use reth_primitives_traits::{NodePrimitives, RecoveredBlock, SignedTransaction};
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory,
    StateCommitmentProvider, StateProviderFactory, StateReader,
};
use reth_revm::db::BundleState;
use reth_trie::TrieInput;
use std::sync::{
    mpsc,
    mpsc::{channel, Sender},
    Arc,
};

mod executor;

mod multiproof;
mod prewarm;
mod sparse_trie;

/// Entrypoint for executing the payload.
pub struct PayloadProcessor<N, Evm> {
    /// The executor used by to spawn tasks.
    executor: WorkloadExecutor,
    /// The most recent cache used for execution.
    execution_cache: ExecutionCache,
    /// Metrics for trie operations
    trie_metrics: StateRootTaskMetrics,
    /// Cross-block cache size in bytes.
    cross_block_cache_size: u64,
    /// Determines how to configure the evm for execution.
    evm_config: Evm,

    _m: std::marker::PhantomData<N>,
}

impl<N, Evm> PayloadProcessor<N, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvmEnvFor<N>
        + 'static
        + ConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>
        + 'static,
{
    /// Executes the payload based on the configured settings.
    pub fn execute(&self) {
        // TODO helpers for executing in sync?
    }

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
    pub fn spawn<P>(
        &self,
        block: RecoveredBlock<N::Block>,
        consistent_view: ConsistentDbView<P>,
        trie_input: TrieInput,
        provider_builder: StateProviderBuilder<N, P>,
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
        let state_root_config = StateRootConfig::new_from_input(consistent_view, trie_input);
        let multi_proof_task =
            StateRootTask2::new(state_root_config.clone(), self.executor.clone(), to_sparse_trie);

        // wire the multiproof task to the prewarm task
        let to_multi_proof = Some(multi_proof_task.state_root_message_sender());

        let prewarm_handle =
            self.spawn_prewarming_with(block, provider_builder, to_multi_proof.clone());

        // spawn multi-proof task
        self.executor.spawn_blocking(move || {
            multi_proof_task.run();
        });

        let sparse_trie_task = SparseTrieTask {
            executor: self.executor.clone(),
            updates: sparse_trie_rx,
            config: state_root_config,
            metrics: self.trie_metrics.clone(),

            // TODO settings
            max_concurrency: 4,
        };

        // wire the sparse trie to the state root response receiver
        let (state_root_tx, state_root_rx) = channel();
        self.executor.spawn_blocking(move || {
            let res = sparse_trie_task.run();
            let _ = state_root_tx.send(res);
        });

        PayloadHandle { to_multi_proof, prewarm_handle, state_root: Some(state_root_rx) }
    }

    /// Spawn prewarming exclusively
    pub fn spawn_prewarming<P>(
        &self,
        block: RecoveredBlock<N::Block>,
        provider_builder: StateProviderBuilder<N, P>,
    ) -> PrewarmTaskHandle
    where
        P: BlockReader
            + StateProviderFactory
            + StateReader
            + StateCommitmentProvider
            + Clone
            + 'static,
    {
        self.spawn_prewarming_with(block, provider_builder, None)
    }

    /// Spawn prewarming optionally wired to the multiproof task for target updates.
    fn spawn_prewarming_with<P>(
        &self,
        block: RecoveredBlock<N::Block>,
        provider_builder: StateProviderBuilder<N, P>,
        to_multi_proof: Option<Sender<StateRootMessage>>,
    ) -> PrewarmTaskHandle
    where
        P: BlockReader
            + StateProviderFactory
            + StateReader
            + StateCommitmentProvider
            + Clone
            + 'static,
    {
        let (cache, cache_metrics) = self.cache_for(block.header().parent_hash()).split();
        // configure prewarming
        let prewarm_ctx = PrewarmContext {
            header: block.clone_sealed_header(),
            evm_config: self.evm_config.clone(),
            cache: cache.clone(),
            cache_metrics,
            provider: provider_builder,
        };

        let txs = block.transactions_recovered().map(Recovered::cloned).collect();
        let prewarm_task = PrewarmTask::new(
            self.executor.clone(),
            self.execution_cache.clone(),
            prewarm_ctx,
            to_multi_proof,
            txs,
        );
        let to_prewarm_task = prewarm_task.actions_tx();

        // spawn pre-warm task
        self.executor.spawn_blocking(move || {
            prewarm_task.run();
        });
        PrewarmTaskHandle { cache, to_prewarm_task: Some(to_prewarm_task) }
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
pub struct PayloadHandle {
    /// Channel for evm state updates
    to_multi_proof: Option<Sender<StateRootMessage>>,
    // must include the receiver of the state root wired to the sparse trie
    prewarm_handle: PrewarmTaskHandle,
    /// Receiver for the state root
    state_root: Option<mpsc::Receiver<StateRootResult>>,
}

impl PayloadHandle {
    /// Awaits the state root
    pub fn state_root(&self) -> StateRootResult {
        todo!()
    }

    // TODO add state hook
    // pub fn state_hook(&self) -> impl OnStateHook {

    /// Terminates the pre-warming transaction processing.
    ///
    /// Note: This does not terminate the task yet.
    pub fn stop_prewarming_execution(&self) {
        self.prewarm_handle.stop_prewarming_execution()
    }

    /// Terminates the entire pre-warming task.
    ///
    /// If the [`BundleState`] is provided it will update the shared cache.
    pub fn terminate_prewarming_execution(&mut self, block_output: Option<BundleState>) {
        self.prewarm_handle.terminate_prewarming_execution(block_output)
    }
}

/// Access to the spawned [`PrewarmTask`].
pub(crate) struct PrewarmTaskHandle {
    /// The shared cache the task operates with.
    cache: ProviderCaches,
    /// Channel to the spawned prewarm task if any
    to_prewarm_task: Option<Sender<PrewarmTaskEvent>>,
}

impl PrewarmTaskHandle {
    /// Terminates the pre-warming transaction processing.
    ///
    /// Note: This does not terminate the task yet.
    pub fn stop_prewarming_execution(&self) {
        self.to_prewarm_task
            .as_ref()
            .map(|tx| tx.send(PrewarmTaskEvent::TerminateTransactionExecution).ok());
    }

    /// Terminates the entire pre-warming task.
    ///
    /// If the [`BundleState`] is provided it will update the shared cache.
    pub fn terminate_prewarming_execution(&mut self, block_output: Option<BundleState>) {
        self.to_prewarm_task
            .take()
            .map(|tx| tx.send(PrewarmTaskEvent::Terminate { block_output }).ok());
    }
}

impl Drop for PrewarmTaskHandle {
    fn drop(&mut self) {
        // Ensure we always terminate on drop
        self.terminate_prewarming_execution(None);
    }
}

/// Shared access to most recently used cache.
///
/// This cache is intended to used for processing the payload in the following manner:
///  - Get Cache if the payload's parent block matches the parent block
///  - Update cache upon successful payload execution
///
/// This process assumes that payloads are received sequentially.
#[derive(Clone, Debug)]
struct ExecutionCache {
    /// Guarded cloneable cache identified by a block hash.
    inner: Arc<RwLock<Option<SavedCache>>>,
}

impl ExecutionCache {
    /// Returns the cache if the currently store cache is for the given `parent_hash`
    pub fn get_cache_for(&self, parent_hash: B256) -> Option<SavedCache> {
        let cache = self.inner.read();
        cache.as_ref().and_then(|cache| {
            if cache.executed_block_hash() == parent_hash {
                Some(cache.clone())
            } else {
                None
            }
        })
    }

    /// Clears the tracked cashe
    pub fn clear(&self) {
        self.inner.write().take();
    }

    /// Stores the provider cache
    pub fn save_cache(&self, cache: SavedCache) {
        self.inner.write().replace(cache);
    }
}
