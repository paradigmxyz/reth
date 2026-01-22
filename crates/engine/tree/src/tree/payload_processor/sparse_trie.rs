//! Sparse Trie task related functionality.

use crate::tree::payload_processor::multiproof::{MultiProofTaskMetrics, SparseTrieUpdate};
use alloy_primitives::B256;
use rayon::iter::{ParallelBridge, ParallelIterator};
use reth_trie::{updates::TrieUpdates, Nibbles};
use reth_trie_parallel::root::ParallelStateRootError;
use reth_trie_sparse::{
    errors::{SparseStateTrieResult, SparseTrieErrorKind},
    provider::{TrieNodeProvider, TrieNodeProviderFactory},
    ClearedSparseStateTrie, SerialSparseTrie, SparseStateTrie, SparseTrieInterface,
};
use smallvec::SmallVec;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use tracing::{debug, debug_span, instrument, trace};

/// A task responsible for populating the sparse trie.
pub(super) struct SparseTrieTask<BPF, A = SerialSparseTrie, S = SerialSparseTrie>
where
    BPF: TrieNodeProviderFactory + Send + Sync,
    BPF::AccountNodeProvider: TrieNodeProvider + Send + Sync,
    BPF::StorageNodeProvider: TrieNodeProvider + Send + Sync,
{
    /// Receives updates from the state root task.
    pub(super) updates: mpsc::Receiver<SparseTrieUpdate>,
    /// `SparseStateTrie` used for computing the state root.
    pub(super) trie: SparseStateTrie<A, S>,
    pub(super) metrics: MultiProofTaskMetrics,
    /// Trie node provider factory.
    blinded_provider_factory: BPF,
}

impl<BPF, A, S> SparseTrieTask<BPF, A, S>
where
    BPF: TrieNodeProviderFactory + Send + Sync + Clone,
    BPF::AccountNodeProvider: TrieNodeProvider + Send + Sync,
    BPF::StorageNodeProvider: TrieNodeProvider + Send + Sync,
    A: SparseTrieInterface + Send + Sync + Default,
    S: SparseTrieInterface + Send + Sync + Default + Clone,
{
    /// Creates a new sparse trie, pre-populating with a [`ClearedSparseStateTrie`].
    pub(super) fn new_with_cleared_trie(
        updates: mpsc::Receiver<SparseTrieUpdate>,
        blinded_provider_factory: BPF,
        metrics: MultiProofTaskMetrics,
        sparse_state_trie: ClearedSparseStateTrie<A, S>,
    ) -> Self {
        Self { updates, metrics, trie: sparse_state_trie.into_inner(), blinded_provider_factory }
    }

    /// Runs the sparse trie task to completion.
    ///
    /// This waits for new incoming [`SparseTrieUpdate`].
    ///
    /// This concludes once the last trie update has been received.
    ///
    /// # Returns
    ///
    /// - State root computation outcome.
    /// - `SparseStateTrie` that needs to be cleared and reused to avoid reallocations.
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    pub(super) fn run(
        mut self,
    ) -> (Result<StateRootComputeOutcome, ParallelStateRootError>, SparseStateTrie<A, S>) {
        // run the main loop to completion
        let result = self.run_inner();
        (result, self.trie)
    }

    /// Inner function to run the sparse trie task to completion.
    ///
    /// See [`Self::run`] for more information.
    fn run_inner(&mut self) -> Result<StateRootComputeOutcome, ParallelStateRootError> {
        let now = Instant::now();

        let mut num_iterations = 0;

        while let Ok(mut update) = self.updates.recv() {
            num_iterations += 1;
            let mut num_updates = 1;
            let _enter =
                debug_span!(target: "engine::tree::payload_processor::sparse_trie", "drain updates")
                    .entered();
            while let Ok(next) = self.updates.try_recv() {
                update.extend(next);
                num_updates += 1;
            }
            drop(_enter);

            debug!(
                target: "engine::root",
                num_updates,
                account_proofs = update.multiproof.account_subtree.len(),
                storage_proofs = update.multiproof.storages.len(),
                "Updating sparse trie"
            );

            let elapsed =
                update_sparse_trie(&mut self.trie, update, &self.blinded_provider_factory)
                    .map_err(|e| {
                        ParallelStateRootError::Other(format!(
                            "could not calculate state root: {e:?}"
                        ))
                    })?;
            self.metrics.sparse_trie_update_duration_histogram.record(elapsed);
            trace!(target: "engine::root", ?elapsed, num_iterations, "Root calculation completed");
        }

        debug!(target: "engine::root", num_iterations, "All proofs processed, ending calculation");

        let start = Instant::now();
        let (state_root, trie_updates) =
            self.trie.root_with_updates(&self.blinded_provider_factory).map_err(|e| {
                ParallelStateRootError::Other(format!("could not calculate state root: {e:?}"))
            })?;

        let end = Instant::now();
        self.metrics.sparse_trie_final_update_duration_histogram.record(end.duration_since(start));
        self.metrics.sparse_trie_total_duration_histogram.record(end.duration_since(now));

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
#[instrument(level = "debug", target = "engine::tree::payload_processor::sparse_trie", skip_all)]
pub(crate) fn update_sparse_trie<BPF, A, S>(
    trie: &mut SparseStateTrie<A, S>,
    SparseTrieUpdate { mut state, multiproof }: SparseTrieUpdate,
    blinded_provider_factory: &BPF,
) -> SparseStateTrieResult<Duration>
where
    BPF: TrieNodeProviderFactory + Send + Sync,
    BPF::AccountNodeProvider: TrieNodeProvider + Send + Sync,
    BPF::StorageNodeProvider: TrieNodeProvider + Send + Sync,
    A: SparseTrieInterface + Send + Sync + Default,
    S: SparseTrieInterface + Send + Sync + Default + Clone,
{
    trace!(target: "engine::root::sparse", "Updating sparse trie");
    let started_at = Instant::now();

    // Reveal new accounts and storage slots.
    trie.reveal_decoded_multiproof(multiproof)?;
    let reveal_multiproof_elapsed = started_at.elapsed();
    trace!(
        target: "engine::root::sparse",
        ?reveal_multiproof_elapsed,
        "Done revealing multiproof"
    );

    // Update storage slots with new values and calculate storage roots.
    let span = tracing::Span::current();
    let results: Vec<_> = state
        .storages
        .into_iter()
        .map(|(address, storage)| (address, storage, trie.take_storage_trie(&address)))
        .par_bridge()
        .map(|(address, storage, storage_trie)| {
            let _enter =
                debug_span!(target: "engine::tree::payload_processor::sparse_trie", parent: &span, "storage trie", ?address)
                    .entered();

            trace!(target: "engine::tree::payload_processor::sparse_trie", "Updating storage");
            let storage_provider = blinded_provider_factory.storage_node_provider(address);
            let mut storage_trie = storage_trie.ok_or(SparseTrieErrorKind::Blind)?;

            if storage.wiped {
                trace!(target: "engine::tree::payload_processor::sparse_trie", "Wiping storage");
                storage_trie.wipe()?;
            }

            // Defer leaf removals until after updates/additions, so that we don't delete an
            // intermediate branch node during a removal and then re-add that branch back during a
            // later leaf addition. This is an optimization, but also a requirement inherited from
            // multiproof generating, which can't know the order that leaf operations happen in.
            let mut removed_slots = SmallVec::<[Nibbles; 8]>::new();

            for (slot, value) in storage.storage {
                let slot_nibbles = Nibbles::unpack(slot);

                if value.is_zero() {
                    removed_slots.push(slot_nibbles);
                    continue;
                }

                trace!(target: "engine::tree::payload_processor::sparse_trie", ?slot_nibbles, "Updating storage slot");
                storage_trie.update_leaf(
                    slot_nibbles,
                    alloy_rlp::encode_fixed_size(&value).to_vec(),
                    &storage_provider,
                )?;
            }

            for slot_nibbles in removed_slots {
                trace!(target: "engine::root::sparse", ?slot_nibbles, "Removing storage slot");
                storage_trie.remove_leaf(&slot_nibbles, &storage_provider)?;
            }

            storage_trie.root();

            SparseStateTrieResult::Ok((address, storage_trie))
        })
        .collect();

    // Defer leaf removals until after updates/additions, so that we don't delete an intermediate
    // branch node during a removal and then re-add that branch back during a later leaf addition.
    // This is an optimization, but also a requirement inherited from multiproof generating, which
    // can't know the order that leaf operations happen in.
    let mut removed_accounts = Vec::new();

    // Update account storage roots
    let _enter =
        tracing::debug_span!(target: "engine::tree::payload_processor::sparse_trie", "account trie")
            .entered();
    for result in results {
        let (address, storage_trie) = result?;
        trie.insert_storage_trie(address, storage_trie);

        if let Some(account) = state.accounts.remove(&address) {
            // If the account itself has an update, remove it from the state update and update in
            // one go instead of doing it down below.
            trace!(target: "engine::root::sparse", ?address, "Updating account and its storage root");
            if !trie.update_account(
                address,
                account.unwrap_or_default(),
                blinded_provider_factory,
            )? {
                removed_accounts.push(address);
            }
        } else if trie.is_account_revealed(address) {
            // Otherwise, if the account is revealed, only update its storage root.
            trace!(target: "engine::root::sparse", ?address, "Updating account storage root");
            if !trie.update_account_storage_root(address, blinded_provider_factory)? {
                removed_accounts.push(address);
            }
        }
    }

    // Update accounts
    for (address, account) in state.accounts {
        trace!(target: "engine::root::sparse", ?address, "Updating account");
        if !trie.update_account(address, account.unwrap_or_default(), blinded_provider_factory)? {
            removed_accounts.push(address);
        }
    }

    // Remove accounts
    for address in removed_accounts {
        trace!(target: "engine::root::sparse", ?address, "Removing account");
        let nibbles = Nibbles::unpack(address);
        trie.remove_account_leaf(&nibbles, blinded_provider_factory)?;
    }

    let elapsed_before = started_at.elapsed();
    trace!(
        target: "engine::root::sparse",
        "Calculating subtries"
    );
    trie.calculate_subtries();

    let elapsed = started_at.elapsed();
    let below_level_elapsed = elapsed - elapsed_before;
    trace!(
        target: "engine::root::sparse",
        ?below_level_elapsed,
        "Intermediate nodes calculated"
    );

    Ok(elapsed)
}
