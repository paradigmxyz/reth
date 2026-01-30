//! Sparse Trie task related functionality.

use crate::tree::{
    multiproof::{
        dispatch_with_chunking, evm_state_to_hashed_post_state, MultiProofMessage,
        VersionedMultiProofTargets,
    },
    payload_processor::multiproof::{MultiProofTaskMetrics, SparseTrieUpdate},
};
use alloy_primitives::B256;
use alloy_rlp::Decodable;
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use rayon::iter::{IntoParallelRefMutIterator, ParallelBridge, ParallelIterator};
use reth_primitives_traits::Account;
use reth_revm::state::EvmState;
use reth_trie::{
    proof_v2::Target, updates::TrieUpdates, DecodedMultiProofV2, HashedPostState, Nibbles,
    TrieAccount, EMPTY_ROOT_HASH,
};
use reth_trie_parallel::{
    proof_task::{
        AccountMultiproofInput, ProofResult, ProofResultContext, ProofResultMessage,
        ProofWorkerHandle,
    },
    root::ParallelStateRootError,
    targets_v2::MultiProofTargetsV2,
};
use reth_trie_sparse::{
    errors::{SparseStateTrieResult, SparseTrieErrorKind, SparseTrieResult},
    provider::{TrieNodeProvider, TrieNodeProviderFactory},
    ClearedSparseStateTrie, LeafUpdate, SerialSparseTrie, SparseStateTrie, SparseTrie,
    SparseTrieExt,
};
use revm_primitives::{hash_map::Entry, B256Map};
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

/// Sparse trie task implementation that uses in-memory sparse trie data to schedule proof fetching.
pub(super) struct SparseTrieCacheTask<A = SerialSparseTrie, S = SerialSparseTrie> {
    /// Sender for proof results.
    proof_result_tx: CrossbeamSender<ProofResultMessage>,
    /// Receiver for proof results directly from workers.
    proof_result_rx: CrossbeamReceiver<ProofResultMessage>,
    /// Receives updates from execution and prewarming.
    updates: CrossbeamReceiver<MultiProofMessage>,
    /// `SparseStateTrie` used for computing the state root.
    trie: SparseStateTrie<A, S>,
    /// Handle to the proof worker pools (storage and account).
    proof_worker_handle: ProofWorkerHandle,
    /// Account trie updates.
    account_updates: B256Map<LeafUpdate>,
    /// Storage trie updates. hashed address -> slot -> update.
    storage_updates: B256Map<B256Map<LeafUpdate>>,
    /// Account updates that are blocked by storage root calculation or account reveal.
    ///
    /// Those are being moved into `account_updates` once storage roots
    /// are revealed and/or calculated.
    ///
    /// Invariant: for each entry in `pending_account_updates` account must either be already
    /// revealed in the trie or have an entry in `account_updates`.
    ///
    /// Values can be either of:
    ///   - None: account had a storage update and is awaiting storage root calculation and/or
    ///     account node reveal to complete.
    ///   - Some(_): account was changed/destroyed and is awaiting storage root calculation/reveal
    ///     to complete.
    pending_account_updates: B256Map<Option<Option<Account>>>,
    /// Cache of account proof targets that were already fetched/requested from the proof workers.
    /// account -> lowest `min_len` requested.
    fetched_account_targets: B256Map<u8>,
    /// Cache of storage proof targets that have already been fetched/requested from the proof
    /// workers. account -> slot -> lowest `min_len` requested.
    fetched_storage_targets: B256Map<B256Map<u8>>,
    /// Whether the last state update has been received.
    finished_state_updates: bool,
    /// Pending targets to be dispatched to the proof workers.
    pending_targets: MultiProofTargetsV2,
    /// Number of pending updates that were received but not yet processed.
    pending_updates: usize,
    /// Metrics for the sparse trie.
    metrics: MultiProofTaskMetrics,
}

impl<A, S> SparseTrieCacheTask<A, S>
where
    A: SparseTrieExt + Default,
    S: SparseTrieExt + Default + Clone,
{
    /// Creates a new sparse trie, pre-populating with a [`ClearedSparseStateTrie`].
    pub(super) fn new_with_cleared_trie(
        updates: CrossbeamReceiver<MultiProofMessage>,
        proof_worker_handle: ProofWorkerHandle,
        metrics: MultiProofTaskMetrics,
        sparse_state_trie: ClearedSparseStateTrie<A, S>,
    ) -> Self {
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        Self {
            proof_result_tx,
            proof_result_rx,
            updates,
            proof_worker_handle,
            trie: sparse_state_trie.into_inner(),
            account_updates: Default::default(),
            storage_updates: Default::default(),
            pending_account_updates: Default::default(),
            fetched_account_targets: Default::default(),
            fetched_storage_targets: Default::default(),
            finished_state_updates: Default::default(),
            pending_targets: Default::default(),
            pending_updates: Default::default(),
            metrics,
        }
    }

    /// Runs the sparse trie task to completion.
    ///
    /// This waits for new incoming [`MultiProofMessage`]s, applies updates to the trie and
    /// schedules proof fetching when needed.
    ///
    /// This concludes once the last state update has been received and processed.
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
                recv(self.updates) -> message => {
                    let update = match message {
                        Ok(m) => m,
                        Err(_) => {
                            break
                        }
                    };

                    self.on_multiproof_message(update);
                    self.pending_updates += 1;
                }
                recv(self.proof_result_rx) -> message => {
                    let Ok(result) = message else {
                        unreachable!("we own the sender half")
                    };
                    let ProofResult::V2(mut result) = result.result? else {
                        unreachable!("sparse trie as cache must only be used with multiproof v2");
                    };

                    while let Ok(res) = self.proof_result_rx.try_recv() {
                        let ProofResult::V2(res) = res.result? else {
                            unreachable!("sparse trie as cache must only be used with multiproof v2");
                        };
                        result.extend(res);
                    }
                    self.on_proof_result(result)?;
                },
            }

            if self.updates.is_empty() || self.pending_updates > 10 {
                self.process_leaf_updates()?;
            }
            
            // Dispatch targets if we have accumulated enough or don't have any pending updates.
            if self.pending_targets.chunking_length() > 100 || self.updates.is_empty() {
                self.dispatch_pending_targets();
            }

            if self.updates.is_empty() && self.proof_result_rx.is_empty() {
                self.dispatch_pending_targets();
                self.process_updates()?;
                self.dispatch_pending_targets();
            }

            if self.finished_state_updates &&
                self.account_updates.is_empty() &&
                self.storage_updates.iter().all(|(_, updates)| updates.is_empty())
            {
                break;
            }
        }

        // Process any remaining pending account updates.
        if !self.pending_account_updates.is_empty() {
            self.process_updates()?;
        }

        debug!(target: "engine::root", "All proofs processed, ending calculation");

        let start = Instant::now();
        let (state_root, trie_updates) =
            self.trie.root_with_updates(&self.proof_worker_handle).map_err(|e| {
                ParallelStateRootError::Other(format!("could not calculate state root: {e:?}"))
            })?;

        let end = Instant::now();
        self.metrics.sparse_trie_final_update_duration_histogram.record(end.duration_since(start));
        self.metrics.sparse_trie_total_duration_histogram.record(end.duration_since(now));

        Ok(StateRootComputeOutcome { state_root, trie_updates })
    }

    fn on_multiproof_message(&mut self, message: MultiProofMessage) {
        match message {
            MultiProofMessage::PrefetchProofs(targets) => self.on_prewarm_targets(targets),
            MultiProofMessage::StateUpdate(_, state) => self.on_state_update(state),
            MultiProofMessage::EmptyProof { .. } => unreachable!(),
            MultiProofMessage::BlockAccessList(_) => todo!(),
            MultiProofMessage::FinishedStateUpdates => self.finished_state_updates = true,
        }
    }

    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn on_prewarm_targets(&mut self, targets: VersionedMultiProofTargets) {
        let VersionedMultiProofTargets::V2(targets) = targets else {
            unreachable!("sparse trie as cache must only be used with V2 multiproof targets");
        };

        for target in targets.account_targets {
            // Only touch accounts that are not yet present in the updates set.
            self.account_updates.entry(target.key()).or_insert(LeafUpdate::Touched);
        }

        for (address, slots) in targets.storage_targets {
            for slot in slots {
                // Only touch storages that are not yet present in the updates set.
                self.storage_updates
                    .entry(address)
                    .or_default()
                    .entry(slot.key())
                    .or_insert(LeafUpdate::Touched);
            }

            // Touch corresponding account leaf to make sure its revealed in accounts trie for
            // storage root update.
            self.account_updates.entry(address).or_insert(LeafUpdate::Touched);
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

            // Make sure account is tracked in `account_updates` so that it is revealed in accounts
            // trie for storage root update.
            self.account_updates.entry(address).or_insert(LeafUpdate::Touched);

            // Make sure account is tracked in `pending_account_updates` so that once storage root
            // is computed, it will be updated in the accounts trie.
            self.pending_account_updates.entry(address).or_insert(None);
        }

        for (address, account) in hashed_state_update.accounts {
            // Track account as touched.
            //
            // This might overwrite an existing update, which is fine, because storage root from it
            // is already tracked in the trie and can be easily fetched again.
            self.account_updates.insert(address, LeafUpdate::Touched);

            // Track account in `pending_account_updates` so that once storage root is computed,
            // it will be updated in the accounts trie.
            self.pending_account_updates.insert(address, Some(account));
        }
    }

    fn on_proof_result(
        &mut self,
        result: DecodedMultiProofV2,
    ) -> Result<(), ParallelStateRootError> {
        self.trie.reveal_decoded_multiproof_v2(result).map_err(|e| {
            ParallelStateRootError::Other(format!("could not reveal multiproof: {e:?}"))
        })
    }

    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn process_leaf_updates(&mut self) -> SparseTrieResult<()> {
        for (address, updates) in &mut self.storage_updates {
            let fetched = self.fetched_storage_targets.entry(*address).or_default();
            let mut targets = Vec::new();

            self.trie.get_or_create_storage_trie_mut(*address).update_leaves(
                updates,
                |path, min_len| match fetched.entry(path) {
                    Entry::Occupied(mut entry) => {
                        if min_len < *entry.get() {
                            entry.insert(min_len);
                            targets.push(Target::new(path).with_min_len(min_len));
                        }
                    }
                    Entry::Vacant(entry) => {
                        entry.insert(min_len);
                        targets.push(Target::new(path).with_min_len(min_len));
                    }
                },
            )?;

            if !targets.is_empty() {
                self.pending_targets.storage_targets.entry(*address).or_default().extend(targets);
            }
        }

        // Process account trie updates and fill the account targets.
        self.trie.trie_mut().update_leaves(
            &mut self.account_updates,
            |target, min_len| match self.fetched_account_targets.entry(target) {
                Entry::Occupied(mut entry) => {
                    if min_len < *entry.get() {
                        entry.insert(min_len);
                        self.pending_targets
                            .account_targets
                            .push(Target::new(target).with_min_len(min_len));
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(min_len);
                    self.pending_targets
                        .account_targets
                        .push(Target::new(target).with_min_len(min_len));
                }
            },
        )?;

        Ok(())
    }

    /// Applies updates to the sparse trie and dispatches requested multiproof targets.
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn process_updates(&mut self) -> SparseTrieResult<()> {
        self.process_leaf_updates()?;

        let roots = self
            .trie
            .storage_tries()
            .par_iter_mut()
            .filter(|(address, _)| {
                self.storage_updates.get(*address).is_some_and(|updates| updates.is_empty())
            })
            .map(|(address, trie)| {
                let root =
                    trie.root().expect("updates are drained, trie should be revealed by now");

                (address, root)
            })
            .collect::<Vec<_>>();

        for (address, storage_root) in roots {
            // If the storage root is known and we have a pending update for this account, encode it
            // into a proper update.
            if let Entry::Occupied(entry) = self.pending_account_updates.entry(*address) &&
                entry.get().is_some()
            {
                let account = entry.remove().expect("just checked, should be Some");
                let encoded = if account.is_none_or(|account| account.is_empty()) &&
                    storage_root == EMPTY_ROOT_HASH
                {
                    Vec::new()
                } else {
                    // TODO: optimize allocation
                    alloy_rlp::encode(account.unwrap_or_default().into_trie_account(storage_root))
                };
                self.account_updates.insert(*address, LeafUpdate::Changed(encoded));
            }
        }

        // Now promote pending account updates if possible.
        self.pending_account_updates.retain(|addr, account| {
            // If account has pending storage updates, it is still pending.
            if self.storage_updates.get(addr).is_some_and(|updates| !updates.is_empty()) {
                return true;
            }

            // Get the current account state either from the trie or from latest account update.
            let trie_account = if let Some(LeafUpdate::Changed(encoded)) = self.account_updates.get(addr) {
                Some(encoded).filter(|encoded| !encoded.is_empty())
            } else if !self.account_updates.contains_key(addr) {
                self.trie.get_account_value(addr)
            } else {
                // Needs to be revealed first
                return true;
            };

            let trie_account = trie_account.map(|value| TrieAccount::decode(&mut &value[..]).expect("invalid account RLP"));

            let (account, storage_root) = if let Some(account) = account.take() {
                // If account is Some(_) here it means it didn't have any storage updates 
                // and we can fetch the storage root directly from the account trie.
                //
                // If it did have storage updates, we would've had processed it above when iterating over storage tries.
                let storage_root = trie_account.map(|account| account.storage_root).unwrap_or(EMPTY_ROOT_HASH);

                (account, storage_root)
            } else {
                (trie_account.map(Into::into), self.trie.storage_root(addr).expect("account had storage updates that were applied to its trie, storage root must be revealed by now"))
            };

            let encoded = if account.is_none_or(|account| account.is_empty()) && storage_root == EMPTY_ROOT_HASH {
                Vec::new()
            } else {
                let account = account.unwrap_or_default().into_trie_account(storage_root);

                // TODO: optimize allocation
                alloy_rlp::encode(account)
            };
            self.account_updates.insert(*addr, LeafUpdate::Changed(encoded));

            false
        });

        // Now process any new leaf updates from promotions above.
        self.process_leaf_updates()?;

        Ok(())
    }

    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn dispatch_pending_targets(&mut self) {
        if !self.pending_targets.is_empty() {
            let chunking_length = self.pending_targets.chunking_length();
            dispatch_with_chunking(
                std::mem::take(&mut self.pending_targets),
                chunking_length,
                Some(60),
                300,
                self.proof_worker_handle.available_account_workers(),
                self.proof_worker_handle.available_storage_workers(),
                MultiProofTargetsV2::chunks,
                |proof_targets| {
                    if let Err(e) = self.proof_worker_handle.dispatch_account_multiproof(
                        AccountMultiproofInput::V2 {
                            targets: proof_targets,
                            proof_result_sender: ProofResultContext::new(
                                self.proof_result_tx.clone(),
                                0,
                                HashedPostState::default(),
                                Instant::now(),
                            ),
                        },
                    ) {
                        error!("failed to dispatch account multiproof: {e:?}");
                    }
                },
            );
        }
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
