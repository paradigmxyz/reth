//! Sparse Trie task related functionality.

use crate::tree::{
    multiproof::{evm_state_to_hashed_post_state, MultiProofMessage, VersionedMultiProofTargets},
    payload_processor::multiproof::{MultiProofTaskMetrics, SparseTrieUpdate},
};
use alloy_primitives::B256;
use alloy_rlp::Decodable;
use crossbeam_channel::Receiver as CrossbeamReceiver;
use rayon::iter::{ParallelBridge, ParallelIterator};
use reth_primitives_traits::Account;
use reth_trie::{updates::TrieUpdates, HashedPostState, Nibbles, TrieAccount, EMPTY_ROOT_HASH};
use reth_trie_parallel::{
    proof_task::{ProofResult, ProofResultMessage, ProofWorkerHandle},
    root::ParallelStateRootError,
    targets_v2::MultiProofTargetsV2,
};
use reth_trie_sparse::{
    errors::{SparseStateTrieResult, SparseTrieErrorKind},
    provider::{TrieNodeProvider, TrieNodeProviderFactory},
    ClearedSparseStateTrie, LeafUpdate, SerialSparseTrie, SparseStateTrie, SparseTrie,
};
use revm_primitives::{hash_map::Entry, B256Map};
use revm_state::EvmState;
use smallvec::SmallVec;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use tracing::{debug, debug_span, error, instrument, trace};

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
    A: SparseTrie + Send + Sync + Default,
    S: SparseTrie + Send + Sync + Default + Clone,
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
                account_proofs = update.multiproof.account_proofs_len(),
                storage_proofs = update.multiproof.storage_proofs_len(),
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

enum PendingAccountUpdate {
    /// An account info update. `None` corresponds to a removed account.
    ///
    /// Inserted into `account_updates` once latest storage root is known.
    Info(Option<Account>),
    /// A storage-only update. `None` means storage root is being calculated.
    ///
    /// Inserted into `account_updates` once storage root is known and account is revealed in the
    /// accounts trie.
    Storage(Option<B256>),
}

pub(super) struct SparseTrieCacheTask<A = SerialSparseTrie, S = SerialSparseTrie> {
    /// Receiver for proof results directly from workers.
    proof_result_rx: CrossbeamReceiver<ProofResultMessage>,
    /// Receives updates from the state root task.
    updates: CrossbeamReceiver<MultiProofMessage>,
    /// `SparseStateTrie` used for computing the state root.
    trie: SparseStateTrie<A, S>,
    /// Handle to the proof worker pools (storage and account).
    proof_worker_handle: ProofWorkerHandle,
    /// Account trie updates.
    account_updates: B256Map<LeafUpdate>,
    /// Storage trie updates. hashed address -> slot -> update.
    storage_updates: B256Map<B256Map<LeafUpdate>>,
    /// Pending account updates awaiting storage root calculation to complete.
    ///
    /// Those are being moved into `account_updates` once storage roots
    /// are revealed and/or calculated.
    ///
    /// Invariant: for each entry in `pending_account_updates` there is a corresponding
    /// [`LeafUpdate::Touched`] entry in `account_updates`.
    pending_account_updates: B256Map<PendingAccountUpdate>,
}

impl<A, S> SparseTrieCacheTask<A, S>
where
    A: SparseTrie + Default,
    S: SparseTrie + Default + Clone,
{
    /// Creates a new sparse trie, pre-populating with a [`ClearedSparseStateTrie`].
    pub(super) fn new_with_cleared_trie(
        updates: mpsc::Receiver<MultiProofMessage>,
        sparse_state_trie: ClearedSparseStateTrie<A, S>,
    ) -> Self {
        Self { updates, trie: sparse_state_trie.into_inner() }
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

        loop {
            crossbeam_channel::select_biased! {
                recv(self.proof_result_rx) -> message => {
                    let result = match message {
                        Ok(m) => m,
                        Err(_) => {
                            continue
                        }
                    };
                    self.on_proof_result(result)?;
                },
                recv(self.updates) -> message => {
                    let message = match message {
                        Ok(m) => m,
                        Err(_) => {
                            continue
                        }
                    };

                    match update {
                        MultiProofMessage::PrefetchProofs(targets) => {
                            self.on_prewarm_targets(targets);
                        }
                        MultiProofMessage::StateUpdate(_, state) => {
                            self.on_state_update(state);
                        }
                        MultiProofMessage::EmptyProof { sequence_number: _, state } => {
                            self.on_hashed_state_update(state);
                        }
                        MultiProofMessage::BlockAccessList(_) => todo!(),
                        MultiProofMessage::FinishedStateUpdates => {}
                    }
                }
            }

            self.process_updates();
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

    fn on_prewarm_targets(&mut self, targets: VersionedMultiProofTargets) {
        let VersionedMultiProofTargets::V2(targets) = targets else {
            unreachable!("sparse trie as cache must only be used with V2 multiproof targets");
        };

        for target in targets.account_targets {
            match self.account_updates.entry(target.key()) {
                Entry::Vacant(vacant) => {
                    vacant.insert(LeafUpdate::Touched);
                }
                Entry::Occupied(_) => {
                    // already touched or changed, no need to override
                }
            }
        }

        for (address, slots) in targets.storage_targets {
            for slot in slots {
                match self.storage_updates.entry(address).or_default().entry(slot.key()) {
                    Entry::Vacant(vacant) => {
                        vacant.insert(LeafUpdate::Touched);
                    }
                    Entry::Occupied(_) => {
                        // already touched or changed, no need to override
                    }
                }
            }

            match self.account_updates.entry(address) {
                Entry::Vacant(vacant) => {
                    // Record account as touched to make sure its revealed in accounts trie for
                    // storage root calculation.
                    vacant.insert(LeafUpdate::Touched);
                }
                Entry::Occupied(_) => {
                    // already touched or changed, no need to override
                }
            }
        }
    }

    /// Processes a state update and encodes all state changes as trie updates.
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all,
        fields(accounts = update.len())
    )]
    fn on_state_update(&mut self, update: EvmState) {
        let hashed_state_update = evm_state_to_hashed_post_state(update);
        self.on_hashed_state_update(hashed_state_update)
    }

    /// Processes a hashed state update and encodes all state changes as trie updates.
    fn on_hashed_state_update(&mut self, hashed_state_update: HashedPostState) {
        for (address, storage) in hashed_state_update.storages {
            for (slot, value) in storage.storage {
                let encoded = if value.is_zero() {
                    Vec::new()
                } else {
                    alloy_rlp::encode_fixed_size(&value).to_vec()
                };
                self.storage_updates
                    .entry(address)
                    .or_default()
                    .insert(slot, LeafUpdate::Changed(encoded));
            }

            match self.account_updates.entry(address) {
                Entry::Vacant(vacant) => {
                    // Record account as touched to make sure its revealed in accounts trie for
                    // storage root calculation.
                    vacant.insert(LeafUpdate::Touched);

                    // Record account in `pending_account_updates` as awaiting the storage root
                    // calculation.
                    self.pending_account_updates
                        .insert(address, PendingAccountUpdate::Storage(None));
                }
                Entry::Occupied(_) => {
                    // already touched or changed, no need to override
                }
            }
        }

        for (address, account) in hashed_state_update.accounts {
            if self.pending_account_updates.contains_key(&address) {
                // overwrite an existing pending account update.
                self.pending_account_updates.insert(address, PendingAccountUpdate::Info(account));
            } else if let Some(storage_root) = self.latest_storage_root_for(&address) {
                // If we have latest storage root for the account, we can encode it into a leaf
                // update right away.
                let encoded = if account.is_none_or(|account| account.is_empty()) &&
                    storage_root == EMPTY_ROOT_HASH
                {
                    Vec::new()
                } else {
                    // TODO: optimize allocation
                    alloy_rlp::encode(&account.unwrap_or_default().into_trie_account(storage_root))
                };

                self.account_updates.insert(address, LeafUpdate::Changed(encoded));
            } else {
                // If we don't have latest storage root for the account, account update is
                // considered pending.
                self.pending_account_updates.insert(address, PendingAccountUpdate::Info(account));

                // Still track account as touched to make sure its revealed in the accounts trie
                // while storage root is being computed.
                self.account_updates.insert(address, LeafUpdate::Touched);
            }
        }
    }

    fn on_proof_result(
        &mut self,
        result: ProofResultMessage,
    ) -> Result<(), ParallelStateRootError> {
        let ProofResult::V2(result) = result.result? else {
            unreachable!("sparse trie as cache must only be used wit hmultiproof v2");
        };

        self.trie.reveal_decoded_multiproof_v2(result).map_err(|e| {
            ParallelStateRootError::Other(format!("could not reveal multiproof: {e:?}"))
        })
    }

    /// Returns whether account's storage root is pending, meaning that account update can't be
    /// encoded yet.
    fn is_storage_root_pending_for(&self, address: &B256) -> bool {
        if let Some(storage_updates) = self.storage_updates.get(address) {
            // If we have pending updates for this address, storage root is still not known
            !storage_updates.is_empty()
        } else {
            // If we didn't have any updates for account yet, we should wait for its node to be
            // revealed in the accounts trie.
            !self.trie.is_account_revealed(*address)
        }
    }

    /// Returns latest known storage root for the given address.
    ///
    /// This will return [`None`] in cases when
    ///   - Account has pending storage updates in `storage_updates`
    ///   - Account did not have any storage updates and was not revealed in the accounts trie yet.
    fn latest_storage_root_for(&mut self, address: &B256) -> Option<B256> {
        if let Some(updates) = self.storage_updates.get(address) {
            if !updates.is_empty() {
                None
            } else {
                self.trie
                    .storage_root(*address)
                    .expect("storage root should be revealed after processing all updates")
                    .into()
            }
        } else {
            if self.trie.is_account_revealed(*address) {
                self.trie
                    .get_account_value(address)
                    .map(|value| {
                        TrieAccount::decode(&mut &value[..])
                            .expect("invalid account RLP")
                            .storage_root
                    })
                    .unwrap_or(EMPTY_ROOT_HASH)
                    .into()
            } else {
                None
            }
        }
    }

    /// Applies updates to the sparse trie and dispathes requested multiproof targets.
    fn process_updates(&mut self) {
        let mut proof_targets = MultiProofTargetsV2::default();

        // Call `update_leaves` for each storage trie and fill the storage targets.
        for (addr, updates) in &mut self.storage_updates {
            let trie = self.trie.get_or_create_storage_trie_mut(*addr);

            // trie.update_leaves(slots, |target| {
            //     proof_targets.storage.entry(addr).or_default().push(target);
            // });

            // If all storage updates were processed, we can now update account trie with the
            // new root.
            if updates.is_empty() {
                let storage_root =
                    trie.root().expect("updates are drained, trie should be revealedby now");

                if let Entry::Occupied(entry) = self.pending_account_updates.entry(*addr) {
                    if matches!(entry.get(), PendingAccountUpdate::Info(_)) {
                        let PendingAccountUpdate::Info(account) = entry.remove() else {
                            unreachable!("just checked, should be an info update");
                        };

                        let encoded = if account.is_none_or(|account| account.is_empty()) &&
                            storage_root == EMPTY_ROOT_HASH
                        {
                            Vec::new()
                        } else {
                            // TODO: optimize allocation
                            alloy_rlp::encode(
                                &account.unwrap_or_default().into_trie_account(storage_root),
                            )
                        };
                        self.account_updates.insert(*addr, LeafUpdate::Changed(encoded));
                    } else {
                        let PendingAccountUpdate::Storage(root) = entry.get_mut() else {
                            unreachable!("just checked, should be a storage update");
                        };

                        // For storage-only updates, set the storage root so that we can combine it
                        // with revealed account info.
                        *root = Some(storage_root);
                    }
                }
            }
        }

        // Now handle pending account updates that had their storage roots revealed in the accounts
        // trie.
        self.pending_account_updates.retain(|addr, account| {
            // If account has pending storage updates, it is still pending.
            if self.storage_updates.get(addr).is_some_and(|updates| !updates.is_empty()) {
                return true;
            }

            // If account was not yet revealed in the accounts trie, we are still missing its
            // storage root.
            if !self.trie.is_account_revealed(*addr) {
                return true;
            }

            let storage_root = if let PendingAccountUpdate::Storage(Some(root)) = account {
                *root
            } else {
                self.trie
                    .get_account_value(addr)
                    .map(|value| {
                        TrieAccount::decode(&mut &value[..])
                            .expect("invalid account RLP")
                            .storage_root
                    })
                    .unwrap_or(EMPTY_ROOT_HASH)
            };

            let account = if let PendingAccountUpdate::Info(account) = account {
                account.take()
            } else {
                self.trie.get_account_value(addr).map(|value| {
                    TrieAccount::decode(&mut &value[..]).expect("invalid account RLP").into()
                })
            };

            let encoded = if account.is_empty() && storage_root == EMPTY_ROOT_HASH {
                Vec::new()
            } else {
                let account = account.unwrap_or_default().into_trie_account(storage_root);

                // TODO: optimize allocation
                alloy_rlp::encode(&account)
            };
            self.account_updates.insert(*addr, LeafUpdate::Changed(encoded));

            false
        });

        // Process account trie updates and fill the account targets.
        // self.trie.update_leaves(&mut self.account_updates, |target| {
        //     proof_targets.account.push(target);
        // });

        // TODO: dispatch to workers
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
    A: SparseTrie + Send + Sync + Default,
    S: SparseTrie + Send + Sync + Default + Clone,
{
    trace!(target: "engine::root::sparse", "Updating sparse trie");
    let started_at = Instant::now();

    // Reveal new accounts and storage slots.
    match multiproof {
        ProofResult::Legacy(decoded, _) => {
            trie.reveal_decoded_multiproof(decoded)?;
        }
        ProofResult::V2(decoded_v2) => {
            trie.reveal_decoded_multiproof_v2(decoded_v2)?;
        }
    }
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
