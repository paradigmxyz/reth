//! Entrypoint for payload processing.

use crate::tree::{
    cached_state::{CachedStateMetrics, CachedStateProvider, ProviderCaches, SavedCache},
    root2::*,
    StateProviderBuilder,
};
use alloy_consensus::{transaction::Recovered, BlockHeader};
use alloy_primitives::B256;
use reth_evm::{ConfigureEvm, ConfigureEvmEnvFor};
use reth_primitives_traits::{header::SealedHeaderFor, NodePrimitives, RecoveredBlock};
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DatabaseProviderFactory, StateCommitmentProvider,
    StateProviderFactory, StateReader,
};
use reth_revm::{database::StateProviderDatabase, state::EvmState};
use reth_trie::TrieInput;
use reth_workload_executor::WorkloadExecutor;
use std::{
    collections::VecDeque,
    sync::{
        mpsc,
        mpsc::{channel, Receiver, Sender},
        Arc, RwLock,
    },
};
use tracing::trace;

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
        let to_prewarm_task = prewarm_task.actions_tx.clone();

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

/// A task responsible for populating the sparse trie.
pub struct SparseTrieTask<F> {
    /// Executor used to spawn subtasks.
    executor: WorkloadExecutor,
    /// Receives updates from the state root task.
    updates: mpsc::Receiver<SparseTrieEvent>,
    // TODO: ideally we need a way to create multiple readers on demand.
    config: StateRootConfig<F>,
    metrics: StateRootTaskMetrics,
    /// How many sparse trie jobs should be executed in parallel
    max_concurrency: usize,
}

impl<F> SparseTrieTask<F>
where
    F: DatabaseProviderFactory<Provider: BlockReader> + StateCommitmentProvider,
{
    /// Runs the sparse trie task to completion.
    ///
    /// This waits for new incoming [`SparseTrieUpdate`].
    ///
    /// This concludes once the last trie update has been received.
    // TODO this should probably return the stateroot as response so we can wire a oneshot channel
    fn run(mut self) -> StateRootResult {
        let mut num_iterations = 0;
        // let mut trie = SparseStateTrie::new(blinded_provider_factory).with_updates(true);

        // TODO setup

        // run
        while let Ok(mut update) = self.updates.recv() {
            match update {
                SparseTrieEvent::Update(_) => {}
                SparseTrieEvent::Processed() => {
                    // TODO apply update to trie, needs shared access?
                }
            }

            // num_iterations += 1;
            // let mut num_updates = 1;
            //
            // while let Ok(next) = self.updates.try_recv() {
            //     // update.extend(next);
            //     // num_updates += 1;
            // }
        }

        todo!()
    }
}

/// The event type the sparse trie task operates on.
pub(crate) enum SparseTrieEvent {
    /// Updates received from the multiproof task.
    ///
    /// This represents a stream of [`SparseTrieUpdate`] where a `None` indicates that all updates
    /// have been received.
    Update(Option<SparseTrieUpdate>),
    /// Updates processed from the spawned trie updates jobs (update_sparse_trie)
    Processed(),
}

/// A task that executes transactions individually in parallel.
pub struct PrewarmTask<N: NodePrimitives, P, Evm> {
    /// The executor used to spawn execution tasks.
    executor: WorkloadExecutor,
    /// Transactions pending execution.
    pending: VecDeque<Recovered<N::SignedTx>>,
    /// Context provided to execution tasks
    ctx: PrewarmContext<N, P, Evm>,
    /// How many txs are currently in progress
    in_progress: usize,
    /// How many transactions should be executed in parallel
    max_concurrency: usize,
    /// Sender to emit evm state outcome messages
    to_multi_proof: mpsc::Sender<StateRootMessage>,
    /// Receiver for events produced by tx execution
    actions_rx: Receiver<PrewarmTaskEvent>,
    /// Sender the transactions use to send their result back
    actions_tx: Sender<PrewarmTaskEvent>,
}

impl<N, P, Evm> PrewarmTask<N, P, Evm>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + StateCommitmentProvider + Clone + 'static,
    Evm: ConfigureEvmEnvFor<N>
        + 'static
        + ConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>
        + 'static,
{
    fn new(
        executor: WorkloadExecutor,
        ctx: PrewarmContext<N, P, Evm>,
        to_multi_proof: mpsc::Sender<StateRootMessage>,
        transactions: VecDeque<Recovered<N::SignedTx>>,
    ) -> Self {
        let (actions_tx, actions_rx) = mpsc::channel();
        Self {
            executor,
            pending: Default::default(),
            ctx,
            in_progress: 0,
            // TODO settings
            max_concurrency: 4,
            to_multi_proof,
            actions_rx,
            actions_tx,
        }
    }

    /// Spawns the next transactions
    fn spawn_next(&mut self) {
        while self.in_progress < self.max_concurrency {
            if let Some(tx) = self.pending.pop_front() {
                self.spawn_transaction(tx);
            } else {
                break
            }
        }
    }

    /// Spawns the given transaction as a blocking task.
    fn spawn_transaction(&mut self, tx: Recovered<N::SignedTx>) {
        let ctx = self.ctx.clone();
        let actions_tx = self.actions_tx.clone();
        self.executor.spawn_blocking(move || {
            ctx.transact(tx);
        });
    }

    fn is_done(&self) -> bool {
        self.in_progress == 0 && self.pending.is_empty()
    }

    /// Executes the task.
    ///
    /// This will execute the transactions until all transactions have been processed or the task
    /// was cancelled.
    fn run(mut self) {
        self.spawn_next();

        while let Ok(event) = self.actions_rx.recv() {
            match event {
                PrewarmTaskEvent::Terminate => {
                    // received terminate signal
                    break
                }
                PrewarmTaskEvent::Outcome { .. } => {}
            }

            self.spawn_next();
            if self.is_done() {
                break
            }
        }
    }
}

/// Context required by tx execution tasks.
#[derive(Debug, Clone)]
struct PrewarmContext<N: NodePrimitives, P, Evm> {
    header: SealedHeaderFor<N>,
    evm_config: Evm,
    caches: ProviderCaches,
    cache_metrics: CachedStateMetrics,
    /// Provider to obtain the state
    provider: StateProviderBuilder<N, P>,
}

impl<N, P, Evm> PrewarmContext<N, P, Evm>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + StateCommitmentProvider + Clone + 'static,
    Evm: ConfigureEvmEnvFor<N>
        + 'static
        + ConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>
        + 'static,
{
    /// Transacts the transaction and returns the state outcome.
    // TODO: proper error handling
    fn transact(mut self, tx: Recovered<N::SignedTx>) -> Option<EvmState> {
        let Self { header, evm_config, caches, cache_metrics, provider } = self;
        // Create the state provider inside the thread
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

        let state_provider = StateProviderDatabase::new(&state_provider);

        let mut evm_env = evm_config.evm_env(&header);

        evm_env.cfg_env.disable_nonce_check = true;

        // create a new executor and disable nonce checks in the env
        let mut evm = evm_config.evm_with_env(state_provider, evm_env);

        // create the tx env and reset nonce
        let tx_env = evm_config.tx_env(&tx);

        todo!()
    }
}

enum PrewarmTaskEvent {
    Terminate,
    Outcome {
        /// Returns the state if the transaction
        state: Option<EvmState>,
    },
}

/// Shared access to most recently used cache.
#[derive(Clone)]
struct ExecutionCache {
    // TODO: simplify internals
    inner: Arc<RwLock<Option<SavedCache>>>,
}
