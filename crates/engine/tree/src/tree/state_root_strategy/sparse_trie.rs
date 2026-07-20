//! Sparse Trie task related functionality.

use std::sync::Arc;

use super::{evm_state_to_hashed_post_state, StateRootComputeOutcome, StateRootMessage};
use alloy_primitives::{
    map::{hash_map::Entry, B256Map},
    B256,
};
use alloy_rlp::{Decodable, Encodable};
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use metrics::{Gauge, Histogram};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use reth_metrics::Metrics;
use reth_primitives_traits::{Account, FastInstant as Instant};
use reth_tasks::Runtime;
use reth_trie::{
    updates::TrieUpdates, DecodedMultiProofV2, HashedPostState, TrieAccount, EMPTY_ROOT_HASH,
    TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_common::{MultiProofTargetsV2, ProofV2Target};
use reth_trie_parallel::{
    error::StateRootTaskError,
    proof_task::{
        AccountMultiproofInput, ProofResultContext, ProofResultMessage, ProofWorkerHandle,
    },
};
use reth_trie_sparse::{
    errors::{SparseStateTrieErrorKind, SparseTrieErrorKind, SparseTrieResult},
    ArenaParallelSparseTrie, DeferredDrops, LeafUpdate, RevealableSparseTrie, SparseStateTrie,
    SparseTrie,
};
use tracing::{debug, debug_span, error, instrument, trace_span};

/// Sparse trie task implementation that uses in-memory sparse trie data to schedule proof fetching.
pub(super) struct SparseTrieCacheTask<A = ArenaParallelSparseTrie, S = ArenaParallelSparseTrie> {
    /// Sender for proof results.
    proof_result_tx: CrossbeamSender<ProofResultMessage>,
    /// Receiver for proof results directly from workers.
    proof_result_rx: CrossbeamReceiver<ProofResultMessage>,
    /// Receives updates from execution and prewarming.
    updates: CrossbeamReceiver<SparseTrieTaskMessage>,
    /// Fires (by disconnecting) when the consumer drops its cancel guard, meaning nobody is
    /// waiting for the result anymore. This is the teardown path for a task whose pending
    /// work never drains, since the updates channel closing is a normal end of stream.
    cancel_rx: CrossbeamReceiver<()>,
    /// Sender half for the channel to send final hashed state to.
    final_hashed_state_tx: Option<std::sync::mpsc::Sender<Arc<HashedPostState>>>,
    /// `SparseStateTrie` used for computing the state root.
    trie: SparseStateTrie<A, S>,
    /// The parent block's state root.
    parent_state_root: B256,
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
    /// Account to the broadest requested parent context (`None` sorts before every known parent).
    fetched_account_targets: B256Map<Option<u8>>,
    /// Cache of storage proof targets that have already been fetched/requested from the proof
    /// workers. Account to slot to the broadest requested parent context.
    fetched_storage_targets: B256Map<B256Map<Option<u8>>>,
    /// Reusable buffer for RLP encoding of accounts.
    account_rlp_buf: Vec<u8>,
    /// Whether the last state update has been received.
    finished_state_updates: bool,
    /// Accumulated account leaf update cache hits.
    account_cache_hits: u64,
    /// Accumulated account leaf update cache misses.
    account_cache_misses: u64,
    /// Accumulated storage leaf update cache hits.
    storage_cache_hits: u64,
    /// Accumulated storage leaf update cache misses.
    storage_cache_misses: u64,
    /// Pending proof targets queued for dispatch to proof workers.
    pending_targets: PendingTargets,
    /// Proof batches dispatched to workers and not yet received.
    in_flight_proof_batches: usize,
    /// Number of pending execution/prewarming updates received but not yet passed to
    /// `update_leaves`.
    pending_updates: usize,
    /// Combined final hashed state.
    ///
    /// Sparse trie task observes and hashes all state updates, allowing it to cheaply construct a
    /// final [`HashedPostState`] and share it with main engine thread without requiring any extra
    /// hashing work.
    final_hashed_state: HashedPostState,
    /// Metrics for the sparse trie.
    metrics: SparseTrieTaskMetrics,
}

impl<A, S> SparseTrieCacheTask<A, S>
where
    A: SparseTrie + Default,
    S: SparseTrie + Default + Clone,
{
    /// Creates a new sparse trie, pre-populating with an existing [`SparseStateTrie`].
    #[expect(clippy::too_many_arguments)]
    pub(super) fn new_with_trie(
        executor: &Runtime,
        updates: CrossbeamReceiver<StateRootMessage>,
        cancel_rx: CrossbeamReceiver<()>,
        final_hashed_state_tx: std::sync::mpsc::Sender<Arc<HashedPostState>>,
        proof_worker_handle: ProofWorkerHandle,
        metrics: SparseTrieTaskMetrics,
        trie: SparseStateTrie<A, S>,
        parent_state_root: B256,
        chunk_size: usize,
    ) -> Self {
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let (hashed_state_tx, hashed_state_rx) = crossbeam_channel::unbounded();

        let parent_span = tracing::Span::current();
        let hashing_metrics = metrics.clone();
        executor.spawn_blocking_named("trie-hashing", move || {
            let _span = trace_span!(parent: parent_span, "run_hashing_task").entered();
            Self::run_hashing_task(updates, hashed_state_tx, hashing_metrics)
        });

        Self {
            proof_result_tx,
            proof_result_rx,
            updates: hashed_state_rx,
            cancel_rx,
            proof_worker_handle,
            final_hashed_state_tx: Some(final_hashed_state_tx),
            trie,
            parent_state_root,
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
            account_cache_hits: 0,
            account_cache_misses: 0,
            storage_cache_hits: 0,
            storage_cache_misses: 0,
            pending_targets: Default::default(),
            in_flight_proof_batches: 0,
            pending_updates: Default::default(),
            final_hashed_state: Default::default(),
            metrics,
        }
    }

    /// Runs the hashing task that drains updates from the channel and converts them to
    /// `HashedPostState` in parallel.
    fn run_hashing_task(
        updates: CrossbeamReceiver<StateRootMessage>,
        hashed_state_tx: CrossbeamSender<SparseTrieTaskMessage>,
        metrics: SparseTrieTaskMetrics,
    ) {
        let mut total_idle_time = std::time::Duration::ZERO;
        let mut idle_start = Instant::now();

        while let Ok(message) = updates.recv() {
            total_idle_time += idle_start.elapsed();

            let msg = match message {
                StateRootMessage::PrefetchProofs(targets) => {
                    SparseTrieTaskMessage::PrefetchProofs(targets)
                }
                StateRootMessage::StateUpdate(state) => {
                    let _span = trace_span!(target: "engine::tree::payload_processor::sparse_trie", "hashing_state_update", n = state.len()).entered();
                    let hashed = evm_state_to_hashed_post_state(state);
                    SparseTrieTaskMessage::HashedState(hashed)
                }
                StateRootMessage::FinishedStateUpdates => {
                    SparseTrieTaskMessage::FinishedStateUpdates
                }
                StateRootMessage::HashedStateUpdate(state) => {
                    SparseTrieTaskMessage::HashedState(state)
                }
            };
            if hashed_state_tx.send(msg).is_err() {
                break;
            }

            idle_start = Instant::now();
        }

        metrics.hashing_task_idle_time_seconds.record(total_idle_time.as_secs_f64());
    }

    /// Returns the trie for reuse in the next payload built on top of this one.
    ///
    /// Should be called after the state root result has been sent.
    pub(super) fn into_trie_for_reuse(self) -> (SparseStateTrie<A, S>, DeferredDrops) {
        let Self { mut trie, .. } = self;
        let deferred = trie.take_deferred_drops();
        (trie, deferred)
    }

    /// Clears the trie, discarding all state.
    ///
    /// Use this when the payload was invalid or cancelled - we don't want to preserve
    /// potentially invalid trie state, but we keep the allocations for reuse.
    pub(super) fn into_cleared_trie(self) -> (SparseStateTrie<A, S>, DeferredDrops) {
        let Self { mut trie, .. } = self;
        trie.clear();
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
    pub(super) fn run(&mut self) -> Result<StateRootComputeOutcome, StateRootTaskError> {
        let now = Instant::now();

        let mut total_idle_time = std::time::Duration::ZERO;
        let mut idle_start = Instant::now();
        let mut done = false;
        let mut finalized_hashed_state = None;

        // Streaming phase: updates are still arriving. Ends when the finish marker is
        // processed. Only producers hold update senders, so the channel closing before the
        // marker means they died without finishing the stream.
        while !self.finished_state_updates {
            let mut t = Instant::now();
            crossbeam_channel::select_biased! {
                recv(self.updates) -> message => {
                    let wake = Instant::now();
                    total_idle_time += wake.duration_since(idle_start);
                    self.metrics
                        .sparse_trie_channel_wait_duration_histogram
                        .record(wake.duration_since(t));

                    let update = message.map_err(|_| StateRootTaskError::Other(
                        "updates channel disconnected before state root calculation".to_string(),
                    ))?;
                    if let Some(hashed_state) = self.on_message(update) {
                        finalized_hashed_state = Some(hashed_state);
                    }
                    self.pending_updates += 1;
                }
                recv(self.proof_result_rx) -> message => {
                    let wake = Instant::now();
                    total_idle_time += wake.duration_since(idle_start);
                    self.metrics
                        .sparse_trie_channel_wait_duration_histogram
                        .record(wake.duration_since(t));
                    t = wake;

                    let Ok(result) = message else {
                        unreachable!("we own the sender half")
                    };
                    self.on_proof_results(result, &mut t)?;
                },
                recv(self.cancel_rx) -> _ => return Err(StateRootTaskError::Canceled),
            }

            done = self.make_progress()?;
            idle_start = Instant::now();
        }

        // Draining phase: the marker is the last message read from the updates channel, so
        // after it only proof results and cancellation can occur. The channel closing when
        // the producers drop their senders is not observed here, and late best-effort hints
        // are ignored: with all updates known, prefetching has nothing left to help.
        while !done {
            let mut t = Instant::now();
            crossbeam_channel::select_biased! {
                recv(self.proof_result_rx) -> message => {
                    let wake = Instant::now();
                    total_idle_time += wake.duration_since(idle_start);
                    self.metrics
                        .sparse_trie_channel_wait_duration_histogram
                        .record(wake.duration_since(t));
                    t = wake;

                    let Ok(result) = message else {
                        unreachable!("we own the sender half")
                    };
                    self.on_proof_results(result, &mut t)?;
                },
                recv(self.cancel_rx) -> _ => return Err(StateRootTaskError::Canceled),
            }

            done = self.make_progress()?;
            idle_start = Instant::now();
        }

        self.metrics.sparse_trie_idle_time_seconds.record(total_idle_time.as_secs_f64());

        debug!(target: "engine::root", "All proofs processed, ending calculation");

        let start = Instant::now();
        let (state_root, trie_updates) = match self.trie.root_with_updates() {
            Ok(result) => result,
            Err(err)
                if matches!(
                    err.kind(),
                    SparseStateTrieErrorKind::Sparse(SparseTrieErrorKind::Blind)
                ) =>
            {
                // A still-blind account trie means this block never changed state, so preserve
                // the cached parent root instead of fetching and revealing
                // the unchanged root node.
                (self.parent_state_root, TrieUpdates::default())
            }
            Err(err) => {
                return Err(StateRootTaskError::Other(format!(
                    "could not calculate state root: {err:?}"
                )))
            }
        };

        #[cfg(feature = "trie-debug")]
        let debug_recorders = self.trie.take_debug_recorders();

        let end = Instant::now();
        self.metrics.sparse_trie_final_update_duration_histogram.record(end.duration_since(start));
        self.metrics.sparse_trie_total_duration_histogram.record(end.duration_since(now));

        self.metrics.sparse_trie_account_cache_hits.record(self.account_cache_hits as f64);
        self.metrics.sparse_trie_account_cache_misses.record(self.account_cache_misses as f64);
        self.metrics.sparse_trie_storage_cache_hits.record(self.storage_cache_hits as f64);
        self.metrics.sparse_trie_storage_cache_misses.record(self.storage_cache_misses as f64);
        self.account_cache_hits = 0;
        self.account_cache_misses = 0;
        self.storage_cache_hits = 0;
        self.storage_cache_misses = 0;

        Ok(StateRootComputeOutcome {
            state_root,
            trie_updates: Arc::new(trie_updates),
            hashed_state: finalized_hashed_state
                .expect("finished state updates publish the hashed post state"),
            #[cfg(feature = "trie-debug")]
            debug_recorders,
        })
    }

    /// Handles a received proof result: coalesces everything already queued, reveals the
    /// proof in the trie, and records timing metrics.
    fn on_proof_results(
        &mut self,
        message: ProofResultMessage,
        t: &mut Instant,
    ) -> Result<(), StateRootTaskError> {
        let mut result = self.on_proof_result_message(message)?;
        while let Ok(next) = self.proof_result_rx.try_recv() {
            let res = self.on_proof_result_message(next)?;
            result.extend(res);
        }

        let phase_end = Instant::now();
        self.metrics
            .sparse_trie_proof_coalesce_duration_histogram
            .record(phase_end.duration_since(*t));
        *t = phase_end;

        self.on_proof_result(result)?;
        self.metrics.sparse_trie_reveal_multiproof_duration_histogram.record(t.elapsed());
        Ok(())
    }

    /// Applies buffered updates to the trie and dispatches proof targets.
    ///
    /// Messages queued after the finish marker are best-effort hints and are not actionable.
    /// Returns `true` once the finish marker was received and all pending trie work is done.
    fn make_progress(&mut self) -> Result<bool, StateRootTaskError> {
        let updates_queued = !self.finished_state_updates && !self.updates.is_empty();

        if !updates_queued && self.proof_result_rx.is_empty() {
            // If we don't have any pending messages, we can spend some time on computing
            // storage roots and promoting account updates.
            self.dispatch_pending_targets()?;
            let t = Instant::now();
            self.process_new_updates()?;
            self.promote_pending_account_updates()?;
            self.metrics.sparse_trie_process_updates_duration_histogram.record(t.elapsed());

            if self.finished_state_updates && !self.has_pending_sparse_trie_updates() {
                return Ok(true);
            }

            self.dispatch_pending_targets()?;
            self.ensure_not_stalled(updates_queued)?;

            // If there's still no pending updates spend some time pre-computing the account
            // trie upper hashes
            if self.proof_result_rx.is_empty() {
                self.trie.calculate_subtries();
            }
        } else if !updates_queued {
            // If we don't have any pending updates, apply them to the trie,
            let t = Instant::now();
            self.process_new_updates()?;
            self.metrics.sparse_trie_process_updates_duration_histogram.record(t.elapsed());
            self.dispatch_pending_targets()?;
        } else if self.pending_targets.len() > self.chunk_size {
            // Make sure to dispatch targets if we've accumulated a lot of them.
            self.dispatch_pending_targets()?;
        }
        Ok(false)
    }

    /// Processes a [`SparseTrieTaskMessage`] from the hashing task.
    fn on_message(&mut self, message: SparseTrieTaskMessage) -> Option<Arc<HashedPostState>> {
        match message {
            SparseTrieTaskMessage::PrefetchProofs(targets) => {
                self.on_prewarm_targets(targets);
                None
            }
            SparseTrieTaskMessage::HashedState(hashed_state) => {
                self.on_hashed_state_update(hashed_state);
                None
            }
            SparseTrieTaskMessage::FinishedStateUpdates => {
                let hashed_state = Arc::new(core::mem::take(&mut self.final_hashed_state));
                let _ = self.final_hashed_state_tx.take().unwrap().send(Arc::clone(&hashed_state));
                self.finished_state_updates = true;
                Some(hashed_state)
            }
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
            if !slots.is_empty() {
                // Look up outer map once per address instead of once per slot.
                let new_updates = self.new_storage_updates.entry(address).or_default();
                for slot in slots {
                    // Only touch storages that are not yet present in the updates set.
                    new_updates.entry(slot.key()).or_insert(LeafUpdate::Touched);
                }
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
        for (&address, storage) in &hashed_state_update.storages {
            if !storage.storage.is_empty() {
                // Look up outer maps once per address instead of once per slot.
                let new_updates = self.new_storage_updates.entry(address).or_default();
                let mut existing_updates = self.storage_updates.get_mut(&address);

                for (&slot, &value) in &storage.storage {
                    let encoded = if value.is_zero() {
                        Vec::new()
                    } else {
                        alloy_rlp::encode_fixed_size(&value).to_vec()
                    };
                    new_updates.insert(slot, LeafUpdate::Changed(encoded));

                    // Remove an existing storage update if it exists.
                    if let Some(ref mut existing) = existing_updates {
                        existing.remove(&slot);
                    }
                }
            }

            // Make sure account is tracked in `account_updates` so that it is revealed in accounts
            // trie for storage root update.
            self.new_account_updates.entry(address).or_insert(LeafUpdate::Touched);

            // Make sure account is tracked in `pending_account_updates` so that once storage root
            // is computed, it will be updated in the accounts trie.
            self.pending_account_updates.entry(address).or_insert(None);
        }

        for (&address, &account) in &hashed_state_update.accounts {
            // Track account as touched.
            //
            // This might overwrite an existing update, which is fine, because storage root from it
            // is already tracked in the trie and can be easily fetched again.
            self.new_account_updates.insert(address, LeafUpdate::Touched);

            // Track account in `pending_account_updates` so that once storage root is computed,
            // it will be updated in the accounts trie.
            self.pending_account_updates.insert(address, Some(account));
        }

        self.final_hashed_state.extend(hashed_state_update);
    }

    fn on_proof_result(&mut self, result: DecodedMultiProofV2) -> Result<(), StateRootTaskError> {
        self.trie
            .reveal_decoded_multiproof_v2(result)
            .map_err(|e| StateRootTaskError::Other(format!("could not reveal multiproof: {e:?}")))
    }

    fn on_proof_result_message(
        &mut self,
        message: ProofResultMessage,
    ) -> Result<DecodedMultiProofV2, StateRootTaskError> {
        debug_assert!(
            self.in_flight_proof_batches > 0,
            "received proof result without an in-flight proof batch"
        );
        self.in_flight_proof_batches = self.in_flight_proof_batches.saturating_sub(1);
        message.result
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
        level = "trace",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn process_leaf_updates(&mut self, new: bool) -> SparseTrieResult<()> {
        let storage_updates =
            if new { &mut self.new_storage_updates } else { &mut self.storage_updates };

        // Process all storage updates, skipping tries with no pending updates.
        let span = trace_span!("process_storage_leaf_updates").entered();
        for (address, updates) in storage_updates {
            if updates.is_empty() {
                continue;
            }
            let _enter = trace_span!(target: "engine::tree::payload_processor::sparse_trie", parent: &span, "storage_trie_leaf_updates", a=%address).entered();

            let trie = self.trie.get_or_create_storage_trie_mut(*address);
            let fetched = self.fetched_storage_targets.entry(*address).or_default();
            let mut targets = Vec::new();

            let updates_len_before = updates.len();
            trie.update_leaves(updates, |path, parent_path_len| match fetched.entry(path) {
                Entry::Occupied(mut entry) => {
                    if parent_path_len < *entry.get() {
                        entry.insert(parent_path_len);
                        targets
                            .push(ProofV2Target::new(path).with_parent_path_len(parent_path_len));
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(parent_path_len);
                    targets.push(ProofV2Target::new(path).with_parent_path_len(parent_path_len));
                }
            })?;
            let updates_len_after = updates.len();
            self.storage_cache_hits += (updates_len_before - updates_len_after) as u64;
            self.storage_cache_misses += updates_len_after as u64;

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
        level = "trace",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn process_account_leaf_updates(&mut self, new: bool) -> SparseTrieResult<bool> {
        let account_updates =
            if new { &mut self.new_account_updates } else { &mut self.account_updates };

        let updates_len_before = account_updates.len();

        self.trie.trie_mut().update_leaves(
            account_updates,
            |target, parent_path_len| match self.fetched_account_targets.entry(target) {
                Entry::Occupied(mut entry) => {
                    if parent_path_len < *entry.get() {
                        entry.insert(parent_path_len);
                        self.pending_targets.push_account_target(
                            ProofV2Target::new(target).with_parent_path_len(parent_path_len),
                        );
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(parent_path_len);
                    self.pending_targets.push_account_target(
                        ProofV2Target::new(target).with_parent_path_len(parent_path_len),
                    );
                }
            },
        )?;

        let updates_len_after = account_updates.len();
        self.account_cache_hits += (updates_len_before - updates_len_after) as u64;
        self.account_cache_misses += updates_len_after as u64;

        Ok(updates_len_after < updates_len_before)
    }

    /// Computes storage roots for accounts whose storage updates are fully drained.
    ///
    /// For each storage trie T that:
    /// 1. was modified in the current block,
    /// 2. all the storage updates are fully drained,
    /// 3. but the storage root hasn't been updated yet,
    ///
    /// we trigger state root computation on a rayon pool.
    fn compute_drained_storage_roots(&mut self) {
        let addresses_to_compute_roots: Vec<_> = self
            .storage_updates
            .iter()
            .filter_map(|(address, updates)| updates.is_empty().then_some(*address))
            .collect();

        struct SendStorageTriePtr<S>(*mut RevealableSparseTrie<S>);
        // SAFETY: this wrapper only forwards the pointer across rayon; deref invariants are
        // documented at the use site below.
        unsafe impl<S: Send> Send for SendStorageTriePtr<S> {}

        let mut tries_to_compute_roots: Vec<(B256, SendStorageTriePtr<S>)> =
            Vec::with_capacity(addresses_to_compute_roots.len());
        for address in addresses_to_compute_roots {
            if let Some(trie) = self.trie.storage_tries_mut().get_mut(&address) &&
                !trie.is_root_cached()
            {
                tries_to_compute_roots.push((address, SendStorageTriePtr(trie)));
            }
        }

        if tries_to_compute_roots.is_empty() {
            return;
        }

        let parent_span =
            debug_span!("compute_drained_storage_roots", n = tries_to_compute_roots.len());
        tries_to_compute_roots.into_par_iter().for_each(|(address, SendStorageTriePtr(trie))| {
            let span = if tracing::enabled!(tracing::Level::TRACE) {
                debug_span!(
                    target: "engine::tree::payload_processor::sparse_trie",
                    parent: &parent_span,
                    "storage_root",
                    ?address
                )
            } else {
                debug_span!(
                    target: "engine::tree::payload_processor::sparse_trie",
                    parent: &parent_span,
                    "storage_root",
                )
            };
            let _enter = span.entered();
            // SAFETY:
            // - pointers are created from `storage_tries_mut().get_mut(address)` above;
            // - `addresses_to_compute_roots` comes from map iteration, so addresses are unique;
            // - we do not insert/remove entries between pointer collection and use, so pointers
            //   stay valid and map reallocation cannot occur;
            // - each pointer is consumed by at most one rayon task, so no aliasing mutable access.
            unsafe { (*trie).root().expect("updates are drained, trie should be revealed by now") };
        });
    }

    /// Iterates through all storage tries for which all updates were processed, computes their
    /// storage roots, and promotes corresponding pending account updates into proper leaf updates
    /// for accounts trie.
    #[instrument(
        level = "trace",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn promote_pending_account_updates(&mut self) -> SparseTrieResult<()> {
        self.process_leaf_updates(false)?;

        if self.pending_account_updates.is_empty() {
            return Ok(());
        }

        self.compute_drained_storage_roots();

        loop {
            let span = trace_span!("promote_updates", promoted = tracing::field::Empty).entered();
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

    fn dispatch_pending_targets(&mut self) -> Result<(), StateRootTaskError> {
        if self.pending_targets.is_empty() {
            return Ok(())
        }

        let _span = trace_span!("dispatch_pending_targets").entered();
        let (targets, chunking_length) = self.pending_targets.take();
        let mut dispatch_error = None;
        dispatch_with_chunking(
            targets,
            chunking_length,
            self.chunk_size,
            self.max_targets_for_chunking,
            self.proof_worker_handle.has_multiple_idle_account_workers(),
            self.proof_worker_handle.has_multiple_idle_storage_workers(),
            MultiProofTargetsV2::chunks,
            |proof_targets| {
                if dispatch_error.is_some() {
                    return;
                }

                match self.proof_worker_handle.dispatch_account_multiproof(AccountMultiproofInput {
                    targets: proof_targets,
                    proof_result_sender: ProofResultContext::new(
                        self.proof_result_tx.clone(),
                        HashedPostState::default(),
                        Instant::now(),
                    ),
                }) {
                    Ok(()) => {
                        self.in_flight_proof_batches += 1;
                    }
                    Err(e) => {
                        error!("failed to dispatch account multiproof: {e:?}");
                        dispatch_error = Some(StateRootTaskError::ProofDispatch(e));
                    }
                }
            },
        );

        if let Some(error) = dispatch_error {
            return Err(error)
        }

        Ok(())
    }

    fn has_pending_sparse_trie_updates(&self) -> bool {
        !self.account_updates.is_empty() ||
            self.storage_updates.values().any(|updates| !updates.is_empty()) ||
            !self.pending_account_updates.is_empty()
    }

    /// Errors when pending trie updates remain but nothing can deliver them: no update
    /// messages are queued, no proof targets are queued or in flight, and no proof results
    /// are waiting.
    ///
    /// `updates_queued` is passed in instead of reading `self.updates` directly, because in
    /// the draining phase the updates channel is not read anymore and may hold ignored late
    /// hints that must not mask a stall.
    fn ensure_not_stalled(&self, updates_queued: bool) -> Result<(), StateRootTaskError> {
        if self.finished_state_updates &&
            !updates_queued &&
            self.pending_updates == 0 &&
            self.pending_targets.is_empty() &&
            self.in_flight_proof_batches == 0 &&
            self.proof_result_rx.is_empty() &&
            self.has_pending_sparse_trie_updates()
        {
            const MAX_STALLED_PROOF_TARGETS_TO_LOG: usize = 5;

            let mut account_targets = self
                .account_updates
                .keys()
                .map(|target| (*target, self.fetched_account_targets.get(target).copied()))
                .collect::<Vec<_>>();
            account_targets.sort_unstable();
            let account_targets_truncated =
                account_targets.len().saturating_sub(MAX_STALLED_PROOF_TARGETS_TO_LOG);
            account_targets.truncate(MAX_STALLED_PROOF_TARGETS_TO_LOG);

            let mut storage_targets = self
                .storage_updates
                .iter()
                .flat_map(|(address, updates)| {
                    let fetched_targets = self.fetched_storage_targets.get(address);
                    updates.keys().map(move |target| {
                        (
                            *address,
                            *target,
                            fetched_targets.and_then(|targets| targets.get(target)).copied(),
                        )
                    })
                })
                .collect::<Vec<_>>();
            storage_targets.sort_unstable();
            let storage_targets_truncated =
                storage_targets.len().saturating_sub(MAX_STALLED_PROOF_TARGETS_TO_LOG);
            storage_targets.truncate(MAX_STALLED_PROOF_TARGETS_TO_LOG);

            error!(
                ?account_targets,
                account_targets_truncated,
                ?storage_targets,
                storage_targets_truncated,
                "sparse trie task stalled: pending updates remain but no proof targets are queued or in flight"
            );

            if std::env::var_os("RETH_EXIT_ON_SPARSE_TRIE_STALL").is_some() {
                eprintln!(
                    "fatal sparse trie stall: exiting before fallback/persistence can continue"
                );
                std::process::exit(101);
            }

            return Err(StateRootTaskError::Stalled)
        }

        Ok(())
    }
}

/// Metrics recorded by sparse trie and hashing tasks.
#[derive(Metrics, Clone)]
#[metrics(scope = "tree.root")]
pub(super) struct SparseTrieTaskMetrics {
    /// Histogram of durations spent revealing multiproof results into the sparse trie.
    pub(super) sparse_trie_reveal_multiproof_duration_histogram: Histogram,
    /// Histogram of durations spent coalescing multiple proof results from the channel.
    pub(super) sparse_trie_proof_coalesce_duration_histogram: Histogram,
    /// Histogram of durations the event loop spent blocked waiting on channels.
    pub(super) sparse_trie_channel_wait_duration_histogram: Histogram,
    /// Histogram of durations spent processing trie updates and promoting pending accounts.
    pub(super) sparse_trie_process_updates_duration_histogram: Histogram,
    /// Histogram of sparse trie final update durations.
    pub(super) sparse_trie_final_update_duration_histogram: Histogram,
    /// Histogram of sparse trie total durations.
    pub(super) sparse_trie_total_duration_histogram: Histogram,
    /// Time spent preparing the sparse trie for reuse after state root computation.
    pub(super) into_trie_for_reuse_duration_histogram: Histogram,
    /// Time spent waiting for preserved sparse trie cache to become available.
    pub(super) sparse_trie_cache_wait_duration_histogram: Histogram,
    /// Histogram for sparse trie task idle time in seconds (waiting for updates or proof
    /// results). Excludes the final wait after the channel is closed.
    pub(super) sparse_trie_idle_time_seconds: Histogram,
    /// Histogram for hashing task idle time in seconds (waiting for messages from execution).
    /// Excludes the final wait after the channel is closed.
    pub(super) hashing_task_idle_time_seconds: Histogram,

    /// Number of account leaf updates applied without needing a new proof (cache hits).
    pub(super) sparse_trie_account_cache_hits: Histogram,
    /// Number of account leaf updates that required a new proof (cache misses).
    pub(super) sparse_trie_account_cache_misses: Histogram,
    /// Number of storage leaf updates applied without needing a new proof (cache hits).
    pub(super) sparse_trie_storage_cache_hits: Histogram,
    /// Number of storage leaf updates that required a new proof (cache misses).
    pub(super) sparse_trie_storage_cache_misses: Histogram,

    /// Retained memory of the preserved sparse trie cache in bytes.
    pub(super) sparse_trie_retained_memory_bytes: Gauge,
    /// Number of storage tries retained in the preserved sparse trie cache.
    pub(super) sparse_trie_retained_storage_tries: Gauge,
}

/// The default max targets, for limiting the number of account and storage proof targets to be
/// fetched by a single worker. If exceeded, chunking is forced regardless of worker availability.
const DEFAULT_MAX_TARGETS_FOR_CHUNKING: usize = 300;

/// Dispatches work items as a single unit or in chunks based on target size and worker
/// availability.
#[expect(clippy::too_many_arguments)]
fn dispatch_with_chunking<T, I>(
    items: T,
    chunking_len: usize,
    chunk_size: usize,
    max_targets_for_chunking: usize,
    has_multiple_idle_account_workers: bool,
    has_multiple_idle_storage_workers: bool,
    chunker: impl FnOnce(T, usize) -> I,
    mut dispatch: impl FnMut(T),
) where
    I: IntoIterator<Item = T>,
{
    let has_full_chunks = chunking_len >= chunk_size.saturating_mul(2);
    let should_chunk = chunking_len > max_targets_for_chunking ||
        (has_full_chunks &&
            (has_multiple_idle_account_workers || has_multiple_idle_storage_workers));

    if should_chunk && chunking_len > chunk_size {
        for chunk in chunker(items, chunk_size) {
            dispatch(chunk);
        }
        return;
    }

    dispatch(items);
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
    fn push_account_target(&mut self, target: ProofV2Target) {
        self.targets.account_targets.push(target);
        self.len += 1;
    }

    /// Extends storage targets for the given address.
    fn extend_storage_targets(&mut self, address: &B256, targets: Vec<ProofV2Target>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, Address, B256, U256};
    use reth_provider::{
        providers::{OverlayBuilder, OverlayStateProviderFactory},
        test_utils::create_test_provider_factory,
        ChainSpecProvider,
    };
    use reth_trie_db::ChangesetCache;
    use reth_trie_parallel::proof_task::ProofTaskCtx;
    use reth_trie_sparse::ArenaParallelSparseTrie;

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
            SparseTrieCacheTask::<ArenaParallelSparseTrie, ArenaParallelSparseTrie>::run_hashing_task(
                updates_rx,
                hashed_state_tx,
                SparseTrieTaskMetrics::default(),
            );
        });

        updates_tx.send(StateRootMessage::HashedStateUpdate(hashed_state)).unwrap();
        updates_tx.send(StateRootMessage::FinishedStateUpdates).unwrap();
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

    #[test]
    fn run_returns_parent_root_without_revealing_blind_trie_when_no_state_updates() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = provider_factory.chain_spec().genesis_hash();
        let overlay_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayBuilder::<reth_chain_state::EthPrimitives>::new(
                anchor_hash,
                ChangesetCache::new(),
            ),
        );
        let proof_worker_handle =
            ProofWorkerHandle::new(&runtime, ProofTaskCtx::new(overlay_factory), false);

        let default_trie = RevealableSparseTrie::blind_from(ArenaParallelSparseTrie::default());
        let trie = SparseStateTrie::default()
            .with_accounts_trie(default_trie.clone())
            .with_default_storage_trie(default_trie)
            .with_updates(true);

        let parent_state_root = B256::from([0x55; 32]);
        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (_cancel_guard, cancel_rx) = crossbeam_channel::bounded::<()>(0);
        let mut task = SparseTrieCacheTask::new_with_trie(
            &runtime,
            updates_rx,
            cancel_rx,
            std::sync::mpsc::channel().0,
            proof_worker_handle,
            SparseTrieTaskMetrics::default(),
            trie,
            parent_state_root,
            1,
        );

        updates_tx.send(StateRootMessage::FinishedStateUpdates).unwrap();
        drop(updates_tx);

        let outcome = task.run().expect("state root computation should succeed");

        assert_eq!(outcome.state_root, parent_state_root);
        assert!(outcome.trie_updates.is_empty());
        assert!(task.trie.state_trie_ref().is_none(), "blind trie should not be revealed");
    }

    #[test]
    fn stall_check_waits_for_in_flight_proofs_then_reports_pending_updates() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = provider_factory.chain_spec().genesis_hash();
        let overlay_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayBuilder::<reth_chain_state::EthPrimitives>::new(
                anchor_hash,
                ChangesetCache::new(),
            ),
        );
        let proof_worker_handle =
            ProofWorkerHandle::new(&runtime, ProofTaskCtx::new(overlay_factory), false);

        let default_trie = RevealableSparseTrie::blind_from(ArenaParallelSparseTrie::default());
        let trie = SparseStateTrie::default()
            .with_accounts_trie(default_trie.clone())
            .with_default_storage_trie(default_trie)
            .with_updates(true);

        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (_cancel_guard, cancel_rx) = crossbeam_channel::bounded::<()>(0);
        let mut task = SparseTrieCacheTask::new_with_trie(
            &runtime,
            updates_rx,
            cancel_rx,
            std::sync::mpsc::channel().0,
            proof_worker_handle,
            SparseTrieTaskMetrics::default(),
            trie,
            B256::from([0x55; 32]),
            1,
        );

        drop(updates_tx);

        let account = B256::from([0x11; 32]);
        let slot = B256::from([0x22; 32]);
        let account_target = B256::from([0x33; 32]);
        let storage_target = B256::from([0x44; 32]);

        task.finished_state_updates = true;
        task.account_updates.insert(account, LeafUpdate::Touched);
        task.storage_updates.entry(account).or_default().insert(slot, LeafUpdate::Touched);
        task.pending_account_updates.insert(account, None);
        task.fetched_account_targets.insert(account_target, None);
        task.fetched_storage_targets.entry(account).or_default().insert(storage_target, Some(11));
        task.in_flight_proof_batches = 1;

        assert!(task.ensure_not_stalled(false).is_ok());

        let result = ProofResultMessage {
            result: Ok(DecodedMultiProofV2::default()),
            elapsed: std::time::Duration::ZERO,
            state: HashedPostState::default(),
        };
        task.on_proof_result_message(result).expect("proof result should be ok");

        assert_eq!(task.in_flight_proof_batches, 0);
        let error = task.ensure_not_stalled(false).expect_err("task should be stalled");
        assert!(matches!(error, StateRootTaskError::Stalled));
        let error = error.to_string();

        assert!(error.contains("sparse trie task stalled"));
        assert!(!error.contains("account_targets"));
        assert!(!error.contains("storage_targets"));
        assert!(!error.contains(&format!("{account:?}")));
        assert!(!error.contains(&format!("{account_target:?}")));
        assert!(!error.contains(&format!("{storage_target:?}")));
        assert!(!error.contains("pending_account_leaves"));
        assert!(!error.contains("pending_storage_leaves"));
        assert!(!error.contains("pending_account_updates"));
        assert!(!error.contains(&format!("{slot:?}")));
    }

    #[test]
    fn run_errors_when_cancel_guard_drops_before_updates_finish() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = provider_factory.chain_spec().genesis_hash();
        let overlay_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayBuilder::<reth_chain_state::EthPrimitives>::new(
                anchor_hash,
                ChangesetCache::new(),
            ),
        );
        let proof_worker_handle =
            ProofWorkerHandle::new(&runtime, ProofTaskCtx::new(overlay_factory), false);

        let default_trie = RevealableSparseTrie::blind_from(ArenaParallelSparseTrie::default());
        let trie = SparseStateTrie::default()
            .with_accounts_trie(default_trie.clone())
            .with_default_storage_trie(default_trie)
            .with_updates(true);

        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (cancel_guard, cancel_rx) = crossbeam_channel::bounded::<()>(0);
        let mut task = SparseTrieCacheTask::new_with_trie(
            &runtime,
            updates_rx,
            cancel_rx,
            std::sync::mpsc::channel().0,
            proof_worker_handle,
            SparseTrieTaskMetrics::default(),
            trie,
            B256::from([0x55; 32]),
            1,
        );

        // The consumer abandons the computation. The updates channel is still open (no finish
        // marker was sent), so without the cancel signal the task would wait forever.
        drop(cancel_guard);

        let error = task.run().expect_err("canceled task must return an error");
        assert!(matches!(error, StateRootTaskError::Canceled));

        drop(updates_tx);
    }

    #[test]
    fn run_ignores_hints_queued_after_updates_finish() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = provider_factory.chain_spec().genesis_hash();
        let overlay_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayBuilder::<reth_chain_state::EthPrimitives>::new(
                anchor_hash,
                ChangesetCache::new(),
            ),
        );
        let proof_worker_handle =
            ProofWorkerHandle::new(&runtime, ProofTaskCtx::new(overlay_factory), false);

        let default_trie = RevealableSparseTrie::blind_from(ArenaParallelSparseTrie::default());
        let trie = SparseStateTrie::default()
            .with_accounts_trie(default_trie.clone())
            .with_default_storage_trie(default_trie)
            .with_updates(true);

        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (cancel_guard, cancel_rx) = crossbeam_channel::bounded::<()>(0);
        let mut task = SparseTrieCacheTask::new_with_trie(
            &runtime,
            updates_rx,
            cancel_rx,
            std::sync::mpsc::channel().0,
            proof_worker_handle,
            SparseTrieTaskMetrics::default(),
            trie,
            B256::from([0x55; 32]),
            1,
        );

        updates_tx.send(StateRootMessage::FinishedStateUpdates).unwrap();
        updates_tx.send(StateRootMessage::PrefetchProofs(Default::default())).unwrap();

        let wait_start = std::time::Instant::now();
        while task.updates.len() < 2 {
            assert!(
                wait_start.elapsed() < std::time::Duration::from_secs(1),
                "hashing task did not queue the test messages"
            );
            std::thread::yield_now();
        }

        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let _ = result_tx.send(task.run());
        });

        let result = result_rx.recv_timeout(std::time::Duration::from_secs(1));
        drop(cancel_guard);
        handle.join().unwrap();

        assert!(result.expect("state root task stalled on a late hint").is_ok());
    }
}
