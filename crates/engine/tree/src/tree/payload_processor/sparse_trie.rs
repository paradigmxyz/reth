//! Contains the implementation of the sparse trie logic responsible for creating the

use crate::tree::root2::{
    SparseTrieUpdate, StateRootComputeOutcome, StateRootConfig, StateRootResult,
    StateRootTaskMetrics,
};
use reth_provider::{BlockReader, DBProvider, DatabaseProviderFactory, StateCommitmentProvider};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof::ProofBlindedProviderFactory,
    trie_cursor::InMemoryTrieCursorFactory,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use reth_trie_parallel::root::ParallelStateRootError;
use reth_trie_sparse::SparseStateTrie;
use reth_workload_executor::WorkloadExecutor;
use std::{sync::mpsc, time::Instant};
use tracing::{debug, trace};

/// A task responsible for populating the sparse trie.
pub struct SparseTrieTask<F> {
    /// Executor used to spawn subtasks.
    pub(super) executor: WorkloadExecutor,
    /// Receives updates from the state root task.
    pub(super) updates: mpsc::Receiver<SparseTrieEvent>,
    // TODO: ideally we need a way to create multiple readers on demand.
    pub(super) config: StateRootConfig<F>,
    pub(super) metrics: StateRootTaskMetrics,
    /// How many sparse trie jobs should be executed in parallel
    pub(super) max_concurrency: usize,
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
    pub(super) fn run(mut self) -> StateRootResult {
        let now = Instant::now();
        let provider_ro = self.config.consistent_view.provider_ro()?;
        let in_memory_trie_cursor = InMemoryTrieCursorFactory::new(
            DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
            &self.config.nodes_sorted,
        );
        let blinded_provider_factory = ProofBlindedProviderFactory::new(
            in_memory_trie_cursor.clone(),
            HashedPostStateCursorFactory::new(
                DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
                &self.config.state_sorted,
            ),
            self.config.prefix_sets.clone(),
        );

        let mut num_iterations = 0;
        let mut trie = SparseStateTrie::new(blinded_provider_factory).with_updates(true);

        while let Ok(mut update) = self.updates.recv() {
            num_iterations += 1;
            let mut num_updates = 1;
            while let Ok(next) = self.updates.try_recv() {
                update.extend(next);
                num_updates += 1;
            }

            debug!(
                target: "engine::root",
                num_updates,
                account_proofs = update.multiproof.account_subtree.len(),
                storage_proofs = update.multiproof.storages.len(),
                "Updating sparse trie"
            );

            let elapsed =
                crate::tree::root2::update_sparse_trie(&mut trie, update).map_err(|e| {
                    ParallelStateRootError::Other(format!("could not calculate state root: {e:?}"))
                })?;
            self.metrics.sparse_trie_update_duration_histogram.record(elapsed);
            trace!(target: "engine::root", ?elapsed, num_iterations, "Root calculation completed");
        }

        debug!(target: "engine::root", num_iterations, "All proofs processed, ending calculation");

        let start = Instant::now();
        let (state_root, trie_updates) = trie.root_with_updates().map_err(|e| {
            ParallelStateRootError::Other(format!("could not calculate state root: {e:?}"))
        })?;
        let elapsed = start.elapsed();

        self.metrics.sparse_trie_final_update_duration_histogram.record(elapsed);

        Ok(StateRootComputeOutcome {
            state_root: (state_root, trie_updates),
            total_time: now.elapsed(),
            time_from_last_update: elapsed,
        })
    }
}

/// Aliased for now to not introduce too many changes at once.
pub type SparseTrieEvent = SparseTrieUpdate;

// /// The event type the sparse trie task operates on.
// pub(crate) enum SparseTrieEvent {
//     /// Updates received from the multiproof task.
//     ///
//     /// This represents a stream of [`SparseTrieUpdate`] where a `None` indicates that all
// updates     /// have been received.
//     Update(Option<SparseTrieUpdate>),
// }
