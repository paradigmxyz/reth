//! Sparse Trie task related functionality.

use std::sync::Arc;

use crate::tree::{
    multiproof::{
        dispatch_with_chunking, evm_state_to_hashed_post_state, MultiProofMessage,
        DEFAULT_MAX_TARGETS_FOR_CHUNKING,
    },
    payload_processor::multiproof::MultiProofTaskMetrics,
};
use alloy_primitives::B256;
use alloy_rlp::{Decodable, Encodable};
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use rayon::iter::ParallelIterator;
use reth_primitives_traits::{Account, FastInstant as Instant, ParallelBridgeBuffered};
use reth_tasks::Runtime;
use reth_trie::{
    proof_v2::Target, updates::TrieUpdates, DecodedMultiProofV2, HashedPostState, TrieAccount,
    EMPTY_ROOT_HASH, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_parallel::{
    proof_task::{
        AccountMultiproofInput, ProofResultContext, ProofResultMessage, ProofWorkerHandle,
    },
    root::ParallelStateRootError,
    targets_v2::MultiProofTargetsV2,
};
#[cfg(feature = "trie-debug")]
use reth_trie_sparse::debug_recorder::TrieDebugRecorder;
use reth_trie_sparse::{
    errors::SparseTrieResult, DeferredDrops, LeafUpdate, ParallelSparseTrie, SparseStateTrie,
    SparseTrie,
};
use revm_primitives::{hash_map::Entry, B256Map};
use tracing::{debug, debug_span, error, instrument, trace_span};

/// Maximum number of pending/prewarm updates that we accumulate in memory before actually applying.
const MAX_PENDING_UPDATES: usize = 100;

/// Sparse trie task implementation that uses in-memory sparse trie data to schedule proof fetching.
pub(super) struct SparseTrieCacheTask<A = ParallelSparseTrie, S = ParallelSparseTrie> {
    /// Sender for proof results.
    proof_result_tx: CrossbeamSender<ProofResultMessage>,
    /// Receiver for proof results directly from workers.
    proof_result_rx: CrossbeamReceiver<ProofResultMessage>,
    /// Receives updates from execution and prewarming.
    updates: CrossbeamReceiver<SparseTrieTaskMessage>,
    /// `SparseStateTrie` used for computing the state root.
    trie: SparseStateTrie<A, S>,
    /// Handle to the proof worker pools (storage and account).
    proof_worker_handle: ProofWorkerHandle,

    /// The size of proof targets chunk to spawn in one calculation.
    /// If None, chunking is disabled and all targets are processed in a single proof.
    chunk_size: usize,
    /// If this number is exceeded and chunking is enabled, then this will override whether or not
    /// there are any active workers and force chunking across workers. This is to prevent tasks
    /// which are very long from hitting a single worker.
    max_targets_for_chunking: usize,

    /// Account trie updates.
    account_updates: B256Map<LeafUpdate>,
    /// Storage trie updates. hashed address -> slot -> update.
    storage_updates: B256Map<B256Map<LeafUpdate>>,

    /// Account updates that are buffered but were not yet applied to the trie.
    new_account_updates: B256Map<LeafUpdate>,
    /// Storage updates that are buffered but were not yet applied to the trie.
    new_storage_updates: B256Map<B256Map<LeafUpdate>>,
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
    /// Reusable buffer for RLP encoding of accounts.
    account_rlp_buf: Vec<u8>,
    /// Whether the last state update has been received.
    finished_state_updates: bool,
    /// Pending proof targets queued for dispatch to proof workers.
    pending_targets: PendingTargets,
    /// Number of pending execution/prewarming updates received but not yet passed to
    /// `update_leaves`.
    pending_updates: usize,

    /// Metrics for the sparse trie.
    metrics: MultiProofTaskMetrics,
}

impl<A, S> SparseTrieCacheTask<A, S>
where
    A: SparseTrie + Default,
    S: SparseTrie + Default + Clone,
{
    /// Creates a new sparse trie, pre-populating with an existing [`SparseStateTrie`].
    pub(super) fn new_with_trie(
        executor: &Runtime,
        updates: CrossbeamReceiver<MultiProofMessage>,
        proof_worker_handle: ProofWorkerHandle,
        metrics: MultiProofTaskMetrics,
        trie: SparseStateTrie<A, S>,
        chunk_size: usize,
    ) -> Self {
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let (hashed_state_tx, hashed_state_rx) = crossbeam_channel::unbounded();

        let parent_span = tracing::Span::current();
        executor.spawn_blocking_named("trie-hashing", move || {
            let _span = debug_span!(parent: parent_span, "run_hashing_task").entered();
            Self::run_hashing_task(updates, hashed_state_tx)
        });

        Self {
            proof_result_tx,
            proof_result_rx,
            updates: hashed_state_rx,
            proof_worker_handle,
            trie,
            chunk_size,
            max_targets_for_chunking: DEFAULT_MAX_TARGETS_FOR_CHUNKING,
            account_updates: Default::default(),
            storage_updates: Default::default(),
            new_account_updates: Default::default(),
            new_storage_updates: Default::default(),
            pending_account_updates: Default::default(),
            fetched_account_targets: Default::default(),
            fetched_storage_targets: Default::default(),
            account_rlp_buf: Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE),
            finished_state_updates: Default::default(),
            pending_targets: Default::default(),
            pending_updates: Default::default(),
            metrics,
        }
    }

    /// Runs the hashing task that drains updates from the channel and converts them to
    /// `HashedPostState` in parallel.
    fn run_hashing_task(
        updates: CrossbeamReceiver<MultiProofMessage>,
        hashed_state_tx: CrossbeamSender<SparseTrieTaskMessage>,
    ) {
        while let Ok(message) = updates.recv() {
            let msg = match message {
                MultiProofMessage::PrefetchProofs(targets) => {
                    SparseTrieTaskMessage::PrefetchProofs(targets)
                }
                MultiProofMessage::StateUpdate(_, state) => {
                    let _span = debug_span!(target: "engine::tree::payload_processor::sparse_trie", "hashing_state_update", n = state.len()).entered();
                    let hashed = evm_state_to_hashed_post_state(state);
                    SparseTrieTaskMessage::HashedState(hashed)
                }
                MultiProofMessage::FinishedStateUpdates => {
                    SparseTrieTaskMessage::FinishedStateUpdates
                }
                MultiProofMessage::EmptyProof { .. } | MultiProofMessage::BlockAccessList(_) => {
                    continue
                }
                MultiProofMessage::HashedStateUpdate(state) => {
                    SparseTrieTaskMessage::HashedState(state)
                }
            };
            if hashed_state_tx.send(msg).is_err() {
                break;
            }
        }
    }

    /// Prunes and shrinks the trie for reuse in the next payload built on top of this one.
    ///
    /// Should be called after the state root result has been sent.
    ///
    /// When `disable_pruning` is true, the trie is preserved without any node pruning,
    /// storage trie eviction, or capacity shrinking, keeping the full cache intact for
    /// benchmarking purposes.
    pub(super) fn into_trie_for_reuse(
        self,
        prune_depth: usize,
        max_storage_tries: usize,
        max_nodes_capacity: usize,
        max_values_capacity: usize,
        disable_pruning: bool,
        updates: &TrieUpdates,
    ) -> (SparseStateTrie<A, S>, DeferredDrops) {
        let Self { mut trie, .. } = self;
        trie.commit_updates(updates);
        if !disable_pruning {
            trie.prune(prune_depth, max_storage_tries);
            trie.shrink_to(max_nodes_capacity, max_values_capacity);
        }
        let deferred = trie.take_deferred_drops();
        (trie, deferred)
    }

    /// Clears and shrinks the trie, discarding all state.
    ///
    /// Use this when the payload was invalid or cancelled - we don't want to preserve
    /// potentially invalid trie state, but we keep the allocations for reuse.
    pub(super) fn into_cleared_trie(
        self,
        max_nodes_capacity: usize,
        max_values_capacity: usize,
    ) -> (SparseStateTrie<A, S>, DeferredDrops) {
        let Self { mut trie, .. } = self;
        trie.clear();
        trie.shrink_to(max_nodes_capacity, max_values_capacity);
        let deferred = trie.take_deferred_drops();
        (trie, deferred)
    }

    /// Runs the sparse trie task to completion.
    ///
    /// This waits for new incoming [`SparseTrieTaskMessage`]s, applies updates
    /// to the trie and schedules proof fetching when needed.
    ///
    /// This concludes once the last state update has been received and processed.
    #[instrument(
        name = "SparseTrieCacheTask::run",
        level = "debug",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    pub(super) fn run(&mut self) -> Result<StateRootComputeOutcome, ParallelStateRootError> {
        let now = Instant::now();

        loop {
            let mut t = Instant::now();
            crossbeam_channel::select_biased! {
                recv(self.updates) -> message => {
                    self.metrics
                        .sparse_trie_channel_wait_duration_histogram
                        .record(t.elapsed());

                    let update = match message {
                        Ok(m) => m,
                        Err(_) => {
                            return Err(ParallelStateRootError::Other(
                                "updates channel disconnected before state root calculation".to_string(),
                            ))
                        }
                    };

                    self.on_message(update);
                    self.pending_updates += 1;
                }
                recv(self.proof_result_rx) -> message => {
                    let phase_end = Instant::now();
                    self.metrics
                        .sparse_trie_channel_wait_duration_histogram
                        .record(phase_end.duration_since(t));
                    t = phase_end;

                    let Ok(result) = message else {
                        unreachable!("we own the sender half")
                    };

                    let mut result = result.result?;
                    while let Ok(next) = self.proof_result_rx.try_recv() {
                        let res = next.result?;
                        result.extend(res);
                    }

                    let phase_end = Instant::now();
                    self.metrics
                        .sparse_trie_proof_coalesce_duration_histogram
                        .record(phase_end.duration_since(t));
                    t = phase_end;

                    self.on_proof_result(result)?;
                    self.metrics
                        .sparse_trie_reveal_multiproof_duration_histogram
                        .record(t.elapsed());
                },
            }

            if self.updates.is_empty() && self.proof_result_rx.is_empty() {
                // If we don't have any pending messages, we can spend some time on computing
                // storage roots and promoting account updates.
                self.dispatch_pending_targets();
                t = Instant::now();
                self.process_new_updates()?;
                self.promote_pending_account_updates()?;
                self.metrics.sparse_trie_process_updates_duration_histogram.record(t.elapsed());

                if self.finished_state_updates &&
                    self.account_updates.is_empty() &&
                    self.storage_updates.iter().all(|(_, updates)| updates.is_empty())
                {
                    break;
                }

                self.dispatch_pending_targets();
            } else if self.updates.is_empty() || self.pending_updates > MAX_PENDING_UPDATES {
                // If we don't have any pending updates OR we've accumulated a lot already, apply
                // them to the trie,
                t = Instant::now();
                self.process_new_updates()?;
                self.metrics.sparse_trie_process_updates_duration_histogram.record(t.elapsed());
                self.dispatch_pending_targets();
            } else if self.pending_targets.len() > self.chunk_size {
                // Make sure to dispatch targets if we've accumulated a lot of them.
                self.dispatch_pending_targets();
            }
        }

        debug!(target: "engine::root", "All proofs processed, ending calculation");

        let start = Instant::now();
        let (state_root, trie_updates) =
            self.trie.root_with_updates(&self.proof_worker_handle).map_err(|e| {
                ParallelStateRootError::Other(format!("could not calculate state root: {e:?}"))
            })?;

        #[cfg(feature = "trie-debug")]
        let debug_recorders = self.trie.take_debug_recorders();

        let end = Instant::now();
        self.metrics.sparse_trie_final_update_duration_histogram.record(end.duration_since(start));
        self.metrics.sparse_trie_total_duration_histogram.record(end.duration_since(now));

        Ok(StateRootComputeOutcome {
            state_root,
            trie_updates: Arc::new(trie_updates),
            #[cfg(feature = "trie-debug")]
            debug_recorders,
        })
    }

    /// Processes a [`SparseTrieTaskMessage`] from the hashing task.
    fn on_message(&mut self, message: SparseTrieTaskMessage) {
        match message {
            SparseTrieTaskMessage::PrefetchProofs(targets) => self.on_prewarm_targets(targets),
            SparseTrieTaskMessage::HashedState(hashed_state) => {
                self.on_hashed_state_update(hashed_state)
            }
            SparseTrieTaskMessage::FinishedStateUpdates => self.finished_state_updates = true,
        }
    }

    #[instrument(
        level = "trace",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn on_prewarm_targets(&mut self, targets: MultiProofTargetsV2) {
        for target in targets.account_targets {
            // Only touch accounts that are not yet present in the updates set.
            self.new_account_updates.entry(target.key()).or_insert(LeafUpdate::Touched);
        }

        for (address, slots) in targets.storage_targets {
            for slot in slots {
                // Only touch storages that are not yet present in the updates set.
                self.new_storage_updates
                    .entry(address)
                    .or_default()
                    .entry(slot.key())
                    .or_insert(LeafUpdate::Touched);
            }

            // Touch corresponding account leaf to make sure its revealed in accounts trie for
            // storage root update.
            self.new_account_updates.entry(address).or_insert(LeafUpdate::Touched);
        }
    }

    /// Processes a hashed state update and encodes all state changes as trie updates.
    #[instrument(
        level = "trace",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn on_hashed_state_update(&mut self, hashed_state_update: HashedPostState) {
        for (address, storage) in hashed_state_update.storages {
            for (slot, value) in storage.storage {
                let encoded = if value.is_zero() {
                    Vec::new()
                } else {
                    alloy_rlp::encode_fixed_size(&value).to_vec()
                };
                self.new_storage_updates
                    .entry(address)
                    .or_default()
                    .insert(slot, LeafUpdate::Changed(encoded));

                // Remove an existing storage update if it exists.
                self.storage_updates.get_mut(&address).and_then(|updates| updates.remove(&slot));
            }

            // Make sure account is tracked in `account_updates` so that it is revealed in accounts
            // trie for storage root update.
            self.new_account_updates.entry(address).or_insert(LeafUpdate::Touched);

            // Make sure account is tracked in `pending_account_updates` so that once storage root
            // is computed, it will be updated in the accounts trie.
            self.pending_account_updates.entry(address).or_insert(None);
        }

        for (address, account) in hashed_state_update.accounts {
            // Track account as touched.
            //
            // This might overwrite an existing update, which is fine, because storage root from it
            // is already tracked in the trie and can be easily fetched again.
            self.new_account_updates.insert(address, LeafUpdate::Touched);

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

    fn process_new_updates(&mut self) -> SparseTrieResult<()> {
        if self.pending_updates == 0 {
            return Ok(());
        }

        let _span = debug_span!("process_new_updates").entered();
        self.pending_updates = 0;

        // Firstly apply all new storage and account updates to the tries.
        self.process_leaf_updates(true)?;

        for (address, mut new) in self.new_storage_updates.drain() {
            match self.storage_updates.entry(address) {
                Entry::Vacant(entry) => {
                    entry.insert(new); // insert the whole map at once, no per-slot loop
                }
                Entry::Occupied(mut entry) => {
                    let updates = entry.get_mut();
                    for (slot, new) in new.drain() {
                        match updates.entry(slot) {
                            Entry::Occupied(mut slot_entry) => {
                                if new.is_changed() {
                                    slot_entry.insert(new);
                                }
                            }
                            Entry::Vacant(slot_entry) => {
                                slot_entry.insert(new);
                            }
                        }
                    }
                }
            }
        }

        for (address, new) in self.new_account_updates.drain() {
            match self.account_updates.entry(address) {
                Entry::Occupied(mut entry) => {
                    if new.is_changed() {
                        entry.insert(new);
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(new);
                }
            }
        }

        Ok(())
    }

    /// Applies all account and storage leaf updates to corresponding tries and collects any new
    /// multiproof targets.
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn process_leaf_updates(&mut self, new: bool) -> SparseTrieResult<()> {
        let storage_updates =
            if new { &mut self.new_storage_updates } else { &mut self.storage_updates };

        // Process all storage updates, skipping tries with no pending updates.
        let span = debug_span!("process_storage_leaf_updates").entered();
        for (address, updates) in storage_updates {
            if updates.is_empty() {
                continue;
            }
            let _enter = trace_span!(target: "engine::tree::payload_processor::sparse_trie", parent: &span, "storage_trie_leaf_updates", a=%address).entered();

            let trie = self.trie.get_or_create_storage_trie_mut(*address);
            let fetched = self.fetched_storage_targets.entry(*address).or_default();
            let mut targets = Vec::new();

            trie.update_leaves(updates, |path, min_len| match fetched.entry(path) {
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
            })?;

            if !targets.is_empty() {
                self.pending_targets.extend_storage_targets(address, targets);
            }
        }

        drop(span);

        // Process account trie updates and fill the account targets.
        self.process_account_leaf_updates(new)?;

        Ok(())
    }

    /// Invokes `update_leaves` for the accounts trie and collects any new targets.
    ///
    /// Returns whether any updates were drained (applied to the trie).
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn process_account_leaf_updates(&mut self, new: bool) -> SparseTrieResult<bool> {
        let account_updates =
            if new { &mut self.new_account_updates } else { &mut self.account_updates };

        let updates_len_before = account_updates.len();

        self.trie.trie_mut().update_leaves(account_updates, |target, min_len| {
            match self.fetched_account_targets.entry(target) {
                Entry::Occupied(mut entry) => {
                    if min_len < *entry.get() {
                        entry.insert(min_len);
                        self.pending_targets
                            .push_account_target(Target::new(target).with_min_len(min_len));
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(min_len);
                    self.pending_targets
                        .push_account_target(Target::new(target).with_min_len(min_len));
                }
            }
        })?;

        Ok(account_updates.len() < updates_len_before)
    }

    /// Iterates through all storage tries for which all updates were processed, computes their
    /// storage roots, and promotes corresponding pending account updates into proper leaf updates
    /// for accounts trie.
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn promote_pending_account_updates(&mut self) -> SparseTrieResult<()> {
        self.process_leaf_updates(false)?;

        if self.pending_account_updates.is_empty() {
            return Ok(());
        }

        let span = debug_span!("compute_storage_roots").entered();
        self
            .trie
            .storage_tries_mut()
            .iter_mut()
            .filter(|(address, trie)| {
                self.storage_updates.get(*address).is_some_and(|updates| updates.is_empty()) &&
                    !trie.is_root_cached()
            })
            .par_bridge_buffered()
            .for_each(|(address, trie)| {
                let _enter = debug_span!(target: "engine::tree::payload_processor::sparse_trie", parent: &span, "storage_root", ?address).entered();
                trie.root().expect("updates are drained, trie should be revealed by now");
            });
        drop(span);

        loop {
            let span = debug_span!("promote_updates", promoted = tracing::field::Empty).entered();
            // Now handle pending account updates that can be upgraded to a proper update.
            let account_rlp_buf = &mut self.account_rlp_buf;
            let mut num_promoted = 0;
            self.pending_account_updates.retain(|addr, account| {
                if let Some(updates) = self.storage_updates.get(addr) {
                    if !updates.is_empty() {
                        // If account has pending storage updates, it is still pending.
                        return true;
                    } else if let Some(account) = account.take() {
                        let storage_root = self.trie.storage_root(addr).expect("updates are drained, storage trie should be revealed by now");
                        let encoded = encode_account_leaf_value(account, storage_root, account_rlp_buf);
                        self.account_updates.insert(*addr, LeafUpdate::Changed(encoded));
                        num_promoted += 1;
                        return false;
                    }
                }

                // Get the current account state either from the trie or from latest account update.
                let trie_account = match self.account_updates.get(addr) {
                    Some(LeafUpdate::Changed(encoded)) => {
                        Some(encoded).filter(|encoded| !encoded.is_empty())
                    }
                    // Needs to be revealed first
                    Some(LeafUpdate::Touched) => return true,
                    None => self.trie.get_account_value(addr),
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

                let encoded = encode_account_leaf_value(account, storage_root, account_rlp_buf);
                self.account_updates.insert(*addr, LeafUpdate::Changed(encoded));
                num_promoted += 1;

                false
            });
            span.record("promoted", num_promoted);
            drop(span);

            // Only exit when no new updates are processed.
            //
            // We need to keep iterating if any updates are being drained because that might
            // indicate that more pending account updates can be promoted.
            if num_promoted == 0 || !self.process_account_leaf_updates(false)? {
                break
            }
        }

        Ok(())
    }

    fn dispatch_pending_targets(&mut self) {
        if self.pending_targets.is_empty() {
            return;
        }

        let _span = debug_span!("dispatch_pending_targets").entered();
        let (targets, chunking_length) = self.pending_targets.take();
        dispatch_with_chunking(
            targets,
            chunking_length,
            self.chunk_size,
            self.max_targets_for_chunking,
            self.proof_worker_handle.available_account_workers(),
            self.proof_worker_handle.available_storage_workers(),
            MultiProofTargetsV2::chunks,
            |proof_targets| {
                if let Err(e) =
                    self.proof_worker_handle.dispatch_account_multiproof(AccountMultiproofInput {
                        targets: proof_targets,
                        proof_result_sender: ProofResultContext::new(
                            self.proof_result_tx.clone(),
                            HashedPostState::default(),
                            Instant::now(),
                        ),
                    })
                {
                    error!("failed to dispatch account multiproof: {e:?}");
                }
            },
        );
    }
}

/// RLP-encodes the account as a [`TrieAccount`] leaf value, or returns empty for deletions.
fn encode_account_leaf_value(
    account: Option<Account>,
    storage_root: B256,
    account_rlp_buf: &mut Vec<u8>,
) -> Vec<u8> {
    if account.is_none_or(|account| account.is_empty()) && storage_root == EMPTY_ROOT_HASH {
        return Vec::new();
    }

    account_rlp_buf.clear();
    account.unwrap_or_default().into_trie_account(storage_root).encode(account_rlp_buf);
    account_rlp_buf.clone()
}

/// Pending proof targets queued for dispatch to proof workers, along with their count.
#[derive(Default)]
struct PendingTargets {
    /// The proof targets.
    targets: MultiProofTargetsV2,
    /// Number of account + storage proof targets currently queued.
    len: usize,
}

impl PendingTargets {
    /// Returns the number of pending targets.
    const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if there are no pending targets.
    const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Takes the pending targets, replacing with empty defaults.
    fn take(&mut self) -> (MultiProofTargetsV2, usize) {
        (std::mem::take(&mut self.targets), std::mem::take(&mut self.len))
    }

    /// Adds a target to the account targets.
    fn push_account_target(&mut self, target: Target) {
        self.targets.account_targets.push(target);
        self.len += 1;
    }

    /// Extends storage targets for the given address.
    fn extend_storage_targets(&mut self, address: &B256, targets: Vec<Target>) {
        self.len += targets.len();
        self.targets.storage_targets.entry(*address).or_default().extend(targets);
    }
}

/// Message type for the sparse trie task.
enum SparseTrieTaskMessage {
    /// A hashed state update ready to be processed.
    HashedState(HashedPostState),
    /// Prefetch proof targets (passed through directly).
    PrefetchProofs(MultiProofTargetsV2),
    /// Signals that all state updates have been received.
    FinishedStateUpdates,
}

/// Outcome of the state root computation, including the state root itself with
/// the trie updates.
#[derive(Debug, Clone)]
pub struct StateRootComputeOutcome {
    /// The state root.
    pub state_root: B256,
    /// The trie updates.
    pub trie_updates: Arc<TrieUpdates>,
    /// Debug recorders taken from the sparse tries, keyed by `None` for account trie
    /// and `Some(address)` for storage tries.
    #[cfg(feature = "trie-debug")]
    pub debug_recorders: Vec<(Option<B256>, TrieDebugRecorder)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, Address, B256, U256};
    use reth_trie_sparse::ParallelSparseTrie;

    #[test]
    fn test_run_hashing_task_hashed_state_update_forwards() {
        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (hashed_state_tx, hashed_state_rx) = crossbeam_channel::unbounded();

        let address = keccak256(Address::random());
        let slot = keccak256(U256::from(42).to_be_bytes::<32>());
        let value = U256::from(999);

        let mut hashed_state = HashedPostState::default();
        hashed_state.accounts.insert(
            address,
            Some(Account { balance: U256::from(100), nonce: 1, bytecode_hash: None }),
        );
        let mut storage = reth_trie::HashedStorage::new(false);
        storage.storage.insert(slot, value);
        hashed_state.storages.insert(address, storage);

        let expected_state = hashed_state.clone();

        let handle = std::thread::spawn(move || {
            SparseTrieCacheTask::<ParallelSparseTrie, ParallelSparseTrie>::run_hashing_task(
                updates_rx,
                hashed_state_tx,
            );
        });

        updates_tx.send(MultiProofMessage::HashedStateUpdate(hashed_state)).unwrap();
        updates_tx.send(MultiProofMessage::FinishedStateUpdates).unwrap();
        drop(updates_tx);

        let SparseTrieTaskMessage::HashedState(received) = hashed_state_rx.recv().unwrap() else {
            panic!("expected HashedState message");
        };

        let account = received.accounts.get(&address).unwrap().unwrap();
        assert_eq!(account.balance, expected_state.accounts[&address].unwrap().balance);
        assert_eq!(account.nonce, expected_state.accounts[&address].unwrap().nonce);

        let storage = received.storages.get(&address).unwrap();
        assert_eq!(*storage.storage.get(&slot).unwrap(), value);

        let second = hashed_state_rx.recv().unwrap();
        assert!(matches!(second, SparseTrieTaskMessage::FinishedStateUpdates));

        assert!(hashed_state_rx.recv().is_err());
        handle.join().unwrap();
    }

    #[test]
    fn test_encode_account_leaf_value_empty_account_and_empty_root_is_empty() {
        let mut account_rlp_buf = vec![0xAB];
        let encoded = encode_account_leaf_value(None, EMPTY_ROOT_HASH, &mut account_rlp_buf);

        assert!(encoded.is_empty());
        // Early return should not touch the caller's buffer.
        assert_eq!(account_rlp_buf, vec![0xAB]);
    }

    #[test]
    fn test_encode_account_leaf_value_non_empty_account_is_rlp() {
        let storage_root = B256::from([0x99; 32]);
        let account = Some(Account {
            nonce: 7,
            balance: U256::from(42),
            bytecode_hash: Some(B256::from([0xAA; 32])),
        });
        let mut account_rlp_buf = vec![0x00, 0x01];

        let encoded = encode_account_leaf_value(account, storage_root, &mut account_rlp_buf);
        let decoded = TrieAccount::decode(&mut &encoded[..]).expect("valid account RLP");

        assert_eq!(decoded.nonce, 7);
        assert_eq!(decoded.balance, U256::from(42));
        assert_eq!(decoded.storage_root, storage_root);
        assert_eq!(account_rlp_buf, encoded);
    }
}
