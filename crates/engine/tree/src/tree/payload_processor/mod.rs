//! Entrypoint for payload processing.

use crate::tree::{
    cached_state::{CachedStateMetrics, ProviderCaches, SavedCache},
    payload_processor::{
        prewarm::{PrewarmContext, PrewarmTask, PrewarmTaskEvent},
        sparse_trie::SparseTrieTask,
    },
    root2::*,
    StateProviderBuilder,
};
use alloy_consensus::{transaction::Recovered, BlockHeader};
use alloy_primitives::B256;
use reth_evm::{
    execute::BlockExecutorProvider, system_calls::OnStateHook, ConfigureEvm, ConfigureEvmEnvFor,
    Evm,
};
use reth_primitives_traits::{NodePrimitives, RecoveredBlock, SignedTransaction};
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory,
    StateCommitmentProvider, StateProviderFactory, StateReader,
};
use reth_trie::TrieInput;
use reth_workload_executor::WorkloadExecutor;
use std::sync::{
    mpsc,
    mpsc::{channel, Sender},
    Arc, RwLock,
};

mod prewarm;
mod sparse_trie;

/// Entrypoint for executing the payload.
pub struct PayloadProcessor<N, Evm> {
    executor: WorkloadExecutor,
    /// The most recent cache used for execution.
    most_recent_cache: ExecutionCache,
    /// Metrics for prewarmed execution
    cache_metrics: CachedStateMetrics,
    /// Metrics for trie operations
    trie_metrics: StateRootTaskMetrics,
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
    ///  - all transaction have been processed
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
    // TODO: needs config to determine how to run this (prewarim optional e.g.)
    pub fn spawn<P>(
        &self,
        block: RecoveredBlock<N::Block>,
        consistent_view: ConsistentDbView<P>,
        trie_input: TrieInput,
        provider_builder: StateProviderBuilder<N, P>,
    ) -> PayloadTaskHandle
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

        let caches = self.cache_for(block.header().parent_hash());
        // configure prewarming
        let prewarm_ctx = PrewarmContext {
            header: block.clone_sealed_header(),
            evm_config: self.evm_config.clone(),
            caches,
            cache_metrics: Default::default(),
            provider: provider_builder,
        };
        // wire the multiproof task to the prewarm task
        let to_multi_proof = multi_proof_task.state_root_message_sender();
        let txs = block.transactions_recovered().map(Recovered::cloned).collect();
        let prewarm_task =
            PrewarmTask::new(self.executor.clone(), prewarm_ctx, to_multi_proof, txs);
        let to_prewarm_task = prewarm_task.actions_tx();

        // spawn pre-warm task
        self.executor.spawn_blocking(move || {
            prewarm_task.run();
        });

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

        PayloadTaskHandle { prewarm: Some(to_prewarm_task), state_root: Some(state_root_rx) }
    }

    /// Returns the cache for the given parent hash.
    ///
    /// If the given hash is different then what is recently cached, the cache will be invalidated.
    fn cache_for(&self, parent_hash: B256) -> ProviderCaches {
        todo!()
    }
}

pub struct PayloadTaskHandle {
    // TODO should internals be an enum to represent no parallel workload

    // must include the receiver of the state root wired to the sparse trie
    prewarm: Option<Sender<PrewarmTaskEvent>>,
    /// Receiver for the state root
    state_root: Option<mpsc::Receiver<StateRootResult>>,
}

impl PayloadTaskHandle {
    /// Awaits the state root
    pub fn state_root(&self) -> StateRootResult {
        todo!()
    }

    /// Terminates the pre-warming processing.
    ///
    /// This will terminate all inprogress tx pre-warm execution.
    pub fn terminate_prewarming(&mut self) {
        self.prewarm.take().map(|tx| tx.send(PrewarmTaskEvent::Terminate).ok());
    }
}

impl Drop for PayloadTaskHandle {
    fn drop(&mut self) {
        // Ensure we drop clean up
        self.terminate_prewarming();
    }
}

/// Shared access to most recently used cache.
#[derive(Clone)]
struct ExecutionCache {
    // TODO: simplify internals
    inner: Arc<RwLock<Option<SavedCache>>>,
}
