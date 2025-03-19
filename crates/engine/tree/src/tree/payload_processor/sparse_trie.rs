//! Sparse Trie task related functionality.

use crate::tree::payload_processor::{
    executor::WorkloadExecutor,
    multiproof::{MultiProofConfig, MultiProofTaskMetrics, SparseTrieUpdate},
};
use alloy_primitives::B256;
use rayon::iter::{ParallelBridge, ParallelIterator};
use reth_provider::{BlockReader, DBProvider, DatabaseProviderFactory, StateCommitmentProvider};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof::ProofBlindedProviderFactory,
    trie_cursor::InMemoryTrieCursorFactory, updates::TrieUpdates, Nibbles,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use reth_trie_parallel::root::ParallelStateRootError;
use reth_trie_sparse::{
    blinded::{BlindedProvider, BlindedProviderFactory},
    errors::{SparseStateTrieResult, SparseTrieErrorKind},
    SparseStateTrie,
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use tracing::{debug, trace, trace_span};

/// The level below which the sparse trie hashes are calculated in
/// [`update_sparse_trie`].
const SPARSE_TRIE_INCREMENTAL_LEVEL: usize = 2;

/// A task responsible for populating the sparse trie.
pub(super) struct SparseTrieTask<F> {
    /// Executor used to spawn subtasks.
    #[allow(unused)] // TODO use this for spawning trie tasks
    pub(super) executor: WorkloadExecutor,
    /// Receives updates from the state root task.
    pub(super) updates: mpsc::Receiver<SparseTrieUpdate>,
    // TODO: ideally we need a way to create multiple readers on demand.
    pub(super) config: MultiProofConfig<F>,
    pub(super) metrics: MultiProofTaskMetrics,
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
    ///
    /// NOTE: This function does not take `self` by value to prevent blocking on [`SparseStateTrie`]
    /// drop.
    pub(super) fn run(&self) -> Result<StateRootComputeOutcome, ParallelStateRootError> {
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
        let mut trie = SparseStateTrie::default().with_updates(true);

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

            let elapsed = update_sparse_trie(&mut trie, update, &blinded_provider_factory)
                .map_err(|e| {
                    ParallelStateRootError::Other(format!("could not calculate state root: {e:?}"))
                })?;
            self.metrics.sparse_trie_update_duration_histogram.record(elapsed);
            trace!(target: "engine::root", ?elapsed, num_iterations, "Root calculation completed");
        }

        debug!(target: "engine::root", num_iterations, "All proofs processed, ending calculation");

        let start = Instant::now();
        let (state_root, trie_updates) =
            trie.root_with_updates(&blinded_provider_factory).map_err(|e| {
                ParallelStateRootError::Other(format!("could not calculate state root: {e:?}"))
            })?;

        self.metrics.sparse_trie_final_update_duration_histogram.record(start.elapsed());
        self.metrics.sparse_trie_total_duration_histogram.record(now.elapsed());

        Ok(StateRootComputeOutcome { state_root, trie_updates })
    }
}

/// Outcome of the state root computation, including the state root itself with
/// the trie updates.
#[derive(Debug)]
pub struct StateRootComputeOutcome {
    /// The state root.
    pub state_root: B256,
    /// The trie updates.
    pub trie_updates: TrieUpdates,
}

/// Updates the sparse trie with the given proofs and state, and returns the elapsed time.
pub(crate) fn update_sparse_trie<BPF>(
    trie: &mut SparseStateTrie,
    SparseTrieUpdate { mut state, multiproof }: SparseTrieUpdate,
    blinded_provider_factory: &BPF,
) -> SparseStateTrieResult<Duration>
where
    BPF: BlindedProviderFactory + Send + Sync,
    BPF::AccountNodeProvider: BlindedProvider + Send + Sync,
    BPF::StorageNodeProvider: BlindedProvider + Send + Sync,
{
    trace!(target: "engine::root::sparse", "Updating sparse trie");
    let started_at = Instant::now();

    // Reveal new accounts and storage slots.
    trie.reveal_multiproof(multiproof)?;
    let reveal_multiproof_elapsed = started_at.elapsed();
    trace!(
        target: "engine::root::sparse",
        ?reveal_multiproof_elapsed,
        "Done revealing multiproof"
    );

    // Update storage slots with new values and calculate storage roots.
    let (tx, rx) = mpsc::channel();
    state
        .storages
        .into_iter()
        .map(|(address, storage)| (address, storage, trie.take_storage_trie(&address)))
        .par_bridge()
        .map(|(address, storage, storage_trie)| {
            let span = trace_span!(target: "engine::root::sparse", "Storage trie", ?address);
            let _enter = span.enter();
            trace!(target: "engine::root::sparse", "Updating storage");
            let mut storage_trie = storage_trie.ok_or(SparseTrieErrorKind::Blind)?;
            let provider = blinded_provider_factory.storage_node_provider(address);

            if storage.wiped {
                trace!(target: "engine::root::sparse", "Wiping storage");
                storage_trie.wipe()?;
            }
            for (slot, value) in storage.storage {
                let slot_nibbles = Nibbles::unpack(slot);
                if value.is_zero() {
                    trace!(target: "engine::root::sparse", ?slot, "Removing storage slot");
                    storage_trie.remove_leaf(&slot_nibbles, &provider)?;
                } else {
                    trace!(target: "engine::root::sparse", ?slot, "Updating storage slot");
                    storage_trie.update_leaf(
                        slot_nibbles,
                        alloy_rlp::encode_fixed_size(&value).to_vec(),
                        &provider,
                    )?;
                }
            }

            storage_trie.root();

            SparseStateTrieResult::Ok((address, storage_trie))
        })
        .for_each_init(|| tx.clone(), |tx, result| tx.send(result).unwrap());
    drop(tx);

    // Update account storage roots
    for result in rx {
        let (address, storage_trie) = result?;
        trie.insert_storage_trie(address, storage_trie);

        if let Some(account) = state.accounts.remove(&address) {
            // If the account itself has an update, remove it from the state update and update in
            // one go instead of doing it down below.
            trace!(target: "engine::root::sparse", ?address, "Updating account and its storage root");
            trie.update_account(address, account.unwrap_or_default(), blinded_provider_factory)?;
        } else if trie.is_account_revealed(address) {
            // Otherwise, if the account is revealed, only update its storage root.
            trace!(target: "engine::root::sparse", ?address, "Updating account storage root");
            trie.update_account_storage_root(address, blinded_provider_factory)?;
        }
    }

    // Update accounts
    for (address, account) in state.accounts {
        trace!(target: "engine::root::sparse", ?address, "Updating account");
        trie.update_account(address, account.unwrap_or_default(), blinded_provider_factory)?;
    }

    let elapsed_before = started_at.elapsed();
    trace!(
        target: "engine::root:sparse",
        level=SPARSE_TRIE_INCREMENTAL_LEVEL,
        "Calculating intermediate nodes below trie level"
    );
    trie.calculate_below_level(SPARSE_TRIE_INCREMENTAL_LEVEL);

    let elapsed = started_at.elapsed();
    let below_level_elapsed = elapsed - elapsed_before;
    trace!(
        target: "engine::root:sparse",
        level=SPARSE_TRIE_INCREMENTAL_LEVEL,
        ?below_level_elapsed,
        "Intermediate nodes calculated"
    );

    Ok(elapsed)
}
