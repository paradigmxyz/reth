//! Entrypoint for payload processing.

use crate::tree::{
    cached_state::{CachedStateMetrics, ProviderCaches},
    root::{SparseTrieUpdate, StateRootConfig, StateRootTaskMetrics},
    StateProviderBuilder,
};
use alloy_consensus::transaction::Recovered;
use reth_primitives_traits::{header::SealedHeaderFor, NodePrimitives};
use reth_provider::{
    BlockReader, DatabaseProviderFactory, HashedPostStateProvider, StateCommitmentProvider,
    StateProviderFactory, StateReader, StateRootProvider,
};
use reth_revm::cancelled::ManualCancel;
use reth_workload_executor::WorkloadExecutor;
use std::{
    collections::VecDeque,
    sync::{
        mpsc,
        mpsc::{Receiver, Sender},
    },
};

/// Entrypoint for starting the background processing
pub struct PayloadProcessor {
    executor: WorkloadExecutor,
}

impl PayloadProcessor {
    /// Spawns all background tasks and returns a handle connected to the tasks.
    ///
    /// - Transaction prewarming task
    /// - State root task
    /// - Sparse trie task
    fn spawn(&self) {

        // TODO batch updates: `on_state_update`
    }
}

pub struct PayloadTaskHandle {
    // TODO should internals be an enum to represent no parallel workload

    // needs receiver to await the stateroot from the background task

    // need channel to emit `StateUpdates` to the state root task

    // On drop this should also terminate the prewarm task
}

impl Drop for PayloadTaskHandle {
    fn drop(&mut self) {
        // TODO: terminate all tasks explicitly
    }
}

/// A task responsible for populating the sparse trie.
pub struct SparseTrieTask<F> {
    executor: WorkloadExecutor,
    /// Receives updates from the state root task
    updates: mpsc::Receiver<SparseTrieUpdate>,
    factory: F,
    config: StateRootConfig<F>,
    metrics: StateRootTaskMetrics,
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
    fn run(mut self) {
        let mut num_iterations = 0;

        // run
        while let Ok(mut update) = self.updates.recv() {
            num_iterations += 1;
            let mut num_updates = 1;

            while let Ok(next) = self.updates.try_recv() {
                update.extend(next);
                num_updates += 1;
            }
        }
    }
}

/// A task that executes transactions individually in parallel.
pub struct PrewarmTask<N: NodePrimitives, P, C> {
    executor: WorkloadExecutor,
    /// Transactions pending execution
    pending: VecDeque<Recovered<N::SignedTx>>,
    /// Context provided to execution tasks
    ctx: PrewarmContext<N, P, C>,
    /// How many txs are currently in progress
    in_progress: usize,
    /// How many transactions should be executed in parallel
    max_concurrency: usize,
    /// Sender to emit Stateroot messages
    to_state_root: (),
    /// Receiver for events produced by tx execution
    actions_rx: Receiver<PrewarmTaskEvent>,

    /// Sender the transactions use to send their result back
    actions_tx: Sender<PrewarmTaskEvent>,
}

impl<N, P, C> PrewarmTask<N, P, C>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + StateCommitmentProvider + Clone,
{
    /// Spawns the next transactions
    fn spawn_next(&mut self) {
        while self.in_progress < self.max_concurrency {
            if let Some(event) = self.pending.pop_front() {
                // TODO spawn the next tx
            } else {
                break
            }
        }
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
            if self.ctx.is_cancelled() {
                // terminate
                break
            }
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
struct PrewarmContext<N: NodePrimitives, P, C> {
    header: SealedHeaderFor<N>,
    evm_config: C,
    caches: ProviderCaches,
    cache_metrics: CachedStateMetrics,
    cancelled: ManualCancel,
    /// Provider to obtain the state
    provider: StateProviderBuilder<N, P>,
}

impl<N: NodePrimitives, P, C> PrewarmContext<N, P, C> {
    /// Returns true if the task is cancelled
    fn is_cancelled(&self) -> bool {
        self.cancelled.is_cancelled()
    }
}

enum PrewarmTaskEvent {
    Terminate,
    Outcome {
        // Evmstate outcome
    },
}
