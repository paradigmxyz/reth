//! Sparse Trie task related functionality.

use std::{
    any::Any,
    panic::{self, AssertUnwindSafe},
    sync::{Arc, OnceLock},
};

use super::{evm_state_to_hashed_post_state, StateRootComputeOutcome, StateRootMessage};
use alloy_primitives::{
    map::{hash_map::Entry, B256Map},
    B256,
};
use alloy_rlp::{Decodable, Encodable};
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use metrics::{Gauge, Histogram};
use reth_metrics::Metrics;
use reth_primitives_traits::{Account, FastInstant as Instant};
use reth_tasks::Runtime;
use reth_trie::{
    updates::TrieUpdates, DecodedMultiProofV2, HashedPostState, TrieAccount, EMPTY_ROOT_HASH,
    TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_common::{MultiProofTargetsV2, ProofTrieNodeV2, ProofV2Target, ProofV2TargetParent};
use reth_trie_parallel::{
    error::StateRootTaskError,
    proof_task::{
        AccountMultiproofInput, ProofResultContext, ProofResultMessage, ProofResultSender,
        ProofWorkerHandle,
    },
};
use reth_trie_sparse::{
    errors::{
        SparseStateTrieErrorKind, SparseStateTrieResult, SparseTrieErrorKind, SparseTrieResult,
    },
    ArenaParallelSparseTrie, BlockedLeafUpdates, DeferredDrops, LeafUpdate, LeafUpdateEvent,
    RevealableSparseTrie, SparseStateTrie, SparseTrie, TrieNodeEpoch,
};
use tracing::{debug, debug_span, error, instrument, trace_span};

/// Sparse trie task implementation that uses in-memory sparse trie data to schedule proof fetching.
pub(super) struct SparseTrieCacheTask<A = ArenaParallelSparseTrie, S = ArenaParallelSparseTrie> {
    /// Sender for proof results.
    proof_result_tx: ProofResultSender,
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
    /// The new epoch assigned to nodes modified by this task.
    new_epoch: TrieNodeEpoch,
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
    /// revealed in the trie, have an entry in `account_updates`, or have a leaf update blocked
    /// inside the accounts trie.
    ///
    /// Values can be either of:
    ///   - None: account had a storage update and is awaiting storage root calculation and/or
    ///     account node reveal to complete.
    ///   - Some(_): account was changed/destroyed and is awaiting storage root calculation/reveal
    ///     to complete.
    pending_account_updates: B256Map<Option<Option<Account>>>,
    /// Account leaf values the accounts trie reported while applying a [`LeafUpdate::Touched`]
    /// entry, so promotion does not have to walk the trie again for an account whose fields did
    /// not change.
    ///
    /// Only accounts awaiting promotion are recorded, and a report never overwrites an existing
    /// entry: it can lag behind an update that promotion already queued but that the trie has
    /// not applied yet. Promotion instead writes the value it just encoded, which is always the
    /// newest one.
    existing_accounts: B256Map<Option<TrieAccount>>,
    /// Accounts from [`Self::pending_account_updates`] that may have become promotable.
    ///
    /// Promotion pops from here instead of scanning every pending account on every round. An
    /// address is queued when its pending entry appears, when its storage trie drains and when
    /// its account leaf update is applied, which are the only transitions that can unblock it.
    /// Queuing an account that is not ready yet is harmless: it is dropped again and re-queued
    /// by the transition that finally unblocks it.
    promotable_accounts: Vec<B256>,
    /// Cache of account proof targets that were already fetched/requested from the proof workers.
    /// Account to the broadest requested parent context (an unknown parent sorts before every
    /// known parent).
    fetched_account_targets: B256Map<FetchedTarget>,
    /// How many proof round trips the account keys of this block needed.
    account_proof_rounds: ProofRounds,
    /// How many proof round trips the storage keys of this block needed, summed over all
    /// addresses when their tries are handed back.
    storage_proof_rounds: ProofRounds,
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
    /// Number of times promotion had to read an account value back from the accounts trie
    /// because [`Self::existing_accounts`] held no report for it.
    account_value_fallbacks: u64,
    /// Pending proof targets queued for dispatch to proof workers.
    pending_targets: PendingTargets,
    /// Proof batches dispatched to workers and not yet received.
    in_flight_proof_batches: usize,
    /// Everything the task knows about a storage trie touched by this block: the trie itself, its
    /// queued leaf updates, the proof nodes waiting to be revealed into it and its proof target
    /// cache. See [`StorageSlot`].
    ///
    /// Invariant: an address is either listed here or has its trie in the state trie's storage
    /// trie map, never both. Everything that reads a storage trie goes through its slot until
    /// [`Self::return_storage_tries`] hands them all back.
    storage: B256Map<StorageSlot<S>>,
    /// Number of slots currently owned by a job running off this thread.
    storage_in_flight: usize,
    /// Sender handed to storage jobs. Kept alive by the task so the receiver never disconnects.
    storage_done_tx: CrossbeamSender<StorageJobMessage<S>>,
    /// Receives storage tries coming back from jobs spawned by this task.
    storage_done_rx: CrossbeamReceiver<StorageJobMessage<S>>,
    /// Number of pending execution/prewarming updates received but not yet passed to
    /// `update_leaves`.
    pending_updates: usize,
    /// Whether the first buffered leaf batch has been applied.
    initial_updates_applied: bool,
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
    S: SparseTrie + Default + Clone + 'static,
{
    /// Creates a new sparse trie, pre-populating with an existing [`SparseStateTrie`].
    #[expect(clippy::too_many_arguments)]
    pub(super) fn new_with_trie(
        executor: &Runtime,
        updates: CrossbeamReceiver<StateRootMessage>,
        cancel_rx: CrossbeamReceiver<()>,
        final_hashed_state_tx: std::sync::mpsc::Sender<Arc<HashedPostState>>,
        proof_worker_handle: ProofWorkerHandle,
        proof_result_tx: ProofResultSender,
        proof_result_rx: CrossbeamReceiver<ProofResultMessage>,
        metrics: SparseTrieTaskMetrics,
        trie: SparseStateTrie<A, S>,
        parent_state_root: B256,
        new_epoch: TrieNodeEpoch,
        chunk_size: usize,
    ) -> Self {
        let (hashed_state_tx, hashed_state_rx) = crossbeam_channel::unbounded();
        let (storage_done_tx, storage_done_rx) = crossbeam_channel::unbounded();

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
            new_epoch,
            chunk_size,
            max_targets_for_chunking: DEFAULT_MAX_TARGETS_FOR_CHUNKING,
            account_updates: Default::default(),
            new_account_updates: Default::default(),
            new_storage_updates: Default::default(),
            pending_account_updates: Default::default(),
            existing_accounts: Default::default(),
            promotable_accounts: Default::default(),
            fetched_account_targets: Default::default(),
            account_proof_rounds: Default::default(),
            storage_proof_rounds: Default::default(),
            account_rlp_buf: Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE),
            finished_state_updates: Default::default(),
            account_cache_hits: 0,
            account_cache_misses: 0,
            storage_cache_hits: 0,
            storage_cache_misses: 0,
            account_value_fallbacks: 0,
            pending_targets: Default::default(),
            in_flight_proof_batches: 0,
            storage: Default::default(),
            storage_in_flight: 0,
            storage_done_tx,
            storage_done_rx,
            pending_updates: Default::default(),
            initial_updates_applied: false,
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
        debug_assert!(
            self.storage.is_empty(),
            "storage tries must be back before the trie is preserved"
        );
        let Self { mut trie, .. } = self;
        let deferred = trie.take_deferred_drops();
        (trie, deferred)
    }

    /// Clears the trie, discarding all state.
    ///
    /// Use this when the payload was invalid or cancelled - we don't want to preserve
    /// potentially invalid trie state, but we keep the allocations for reuse.
    pub(super) fn into_cleared_trie(self) -> (SparseStateTrie<A, S>, DeferredDrops) {
        let Self { mut trie, storage_done_rx, storage, .. } = self;
        // Disconnect first so tries still out with a job are dropped by that job instead of here.
        drop(storage_done_rx);
        for (address, slot) in storage {
            if let StorageSlot::Idle(work) = slot {
                trie.insert_storage_trie(address, work.trie);
            }
        }
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
                recv(self.storage_done_rx) -> message => {
                    let wake = Instant::now();
                    total_idle_time += wake.duration_since(idle_start);
                    self.metrics
                        .sparse_trie_channel_wait_duration_histogram
                        .record(wake.duration_since(t));

                    let Ok(returned) = message else {
                        unreachable!("we own the sender half")
                    };
                    self.on_storage_job_message(returned)?;
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
                recv(self.storage_done_rx) -> message => {
                    let wake = Instant::now();
                    total_idle_time += wake.duration_since(idle_start);
                    self.metrics
                        .sparse_trie_channel_wait_duration_histogram
                        .record(wake.duration_since(t));

                    let Ok(returned) = message else {
                        unreachable!("we own the sender half")
                    };
                    self.on_storage_job_message(returned)?;
                },
                recv(self.cancel_rx) -> _ => return Err(StateRootTaskError::Canceled),
            }

            done = self.make_progress()?;
            idle_start = Instant::now();
        }

        debug_assert_eq!(
            self.storage_in_flight, 0,
            "completion must wait for every checked out storage trie"
        );
        self.metrics.sparse_trie_idle_time_seconds.record(total_idle_time.as_secs_f64());

        debug!(target: "engine::root", "All proofs processed, ending calculation");

        let start = Instant::now();
        self.return_storage_tries();
        let (state_root, trie_updates) = match self.trie.root_with_updates(self.new_epoch) {
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

        let end = Instant::now();
        self.metrics.sparse_trie_final_update_duration_histogram.record(end.duration_since(start));
        self.metrics.sparse_trie_total_duration_histogram.record(end.duration_since(now));

        self.metrics.sparse_trie_account_cache_hits.record(self.account_cache_hits as f64);
        self.metrics.sparse_trie_account_cache_misses.record(self.account_cache_misses as f64);
        self.metrics.sparse_trie_storage_cache_hits.record(self.storage_cache_hits as f64);
        self.metrics.sparse_trie_storage_cache_misses.record(self.storage_cache_misses as f64);
        self.metrics
            .sparse_trie_account_value_fallbacks
            .record(self.account_value_fallbacks as f64);
        self.account_cache_hits = 0;
        self.account_cache_misses = 0;
        self.storage_cache_hits = 0;
        self.storage_cache_misses = 0;
        self.account_value_fallbacks = 0;

        self.record_proof_rounds();

        Ok(StateRootComputeOutcome {
            state_root,
            trie_updates: Arc::new(trie_updates),
            hashed_state: finalized_hashed_state
                .expect("finished state updates publish the hashed post state"),
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
        // Absorb a whole burst of returns before the scans below, so one batch of storage jobs
        // costs one promotion pass rather than one per trie.
        self.drain_returned_storage_tries()?;

        let updates_queued = !self.finished_state_updates && !self.updates.is_empty();

        if !updates_queued && self.proof_result_rx.is_empty() {
            // If we don't have any pending messages, we can spend some time on computing
            // storage roots and promoting account updates.
            self.dispatch_pending_targets()?;
            let t = Instant::now();
            self.process_new_updates()?;
            self.run_ready_storage_work()?;
            self.promote_pending_account_updates()?;
            self.metrics.sparse_trie_process_updates_duration_histogram.record(t.elapsed());

            if self.finished_state_updates && !self.has_pending_leaf_updates() {
                if self.pending_account_updates.is_empty() {
                    return Ok(true);
                }

                // No leaf update is left that could unblock an account, so everything still
                // pending must be promotable. Requeuing all of them turns a transition the
                // ready queue missed into a full scan instead of a stalled task.
                self.promote_all_pending_accounts()?;
                if !self.has_pending_sparse_trie_updates() {
                    return Ok(true);
                }
            }

            self.dispatch_pending_targets()?;
            self.ensure_not_stalled(updates_queued)?;

            // If there's still no pending updates spend some time pre-computing the account
            // trie upper hashes
            if self.proof_result_rx.is_empty() {
                self.trie.calculate_subtries(self.new_epoch);
            }
        } else if !updates_queued {
            // If we don't have any pending updates, apply them to the trie,
            let t = Instant::now();
            self.process_new_updates()?;
            self.run_ready_storage_work()?;
            self.metrics.sparse_trie_process_updates_duration_histogram.record(t.elapsed());
            self.dispatch_pending_targets()?;
        } else if !self.initial_updates_applied && self.pending_updates >= INITIAL_UPDATE_BATCH_SIZE
        {
            // Start proof fetching before a continuously arriving state stream drains. Later
            // batches retain the usual coalescing policy to avoid repeatedly sorting small maps.
            let t = Instant::now();
            self.process_new_updates()?;
            self.run_ready_storage_work()?;
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
                // Look up the outer map once per address instead of once per slot.
                let new_updates = self.new_storage_updates.entry(address).or_default();

                for (&slot, &value) in &storage.storage {
                    let encoded = if value.is_zero() {
                        Vec::new()
                    } else {
                        alloy_rlp::encode_fixed_size(&value).to_vec()
                    };
                    new_updates.insert(slot, LeafUpdate::Changed(encoded));
                }
            }

            // Make sure account is tracked in `account_updates` so that it is revealed in accounts
            // trie for storage root update.
            self.new_account_updates.entry(address).or_insert(LeafUpdate::Touched);

            // Make sure account is tracked in `pending_account_updates` so that once storage root
            // is computed, it will be updated in the accounts trie.
            if let Entry::Vacant(entry) = self.pending_account_updates.entry(address) {
                entry.insert(None);
                self.promotable_accounts.push(address);
            }
        }

        for (&address, &account) in &hashed_state_update.accounts {
            // Track account as touched.
            //
            // This might overwrite an existing update, which is fine, because storage root from it
            // is already tracked in the trie and can be easily fetched again.
            self.new_account_updates.insert(address, LeafUpdate::Touched);

            // Track account in `pending_account_updates` so that once storage root is computed,
            // it will be updated in the accounts trie.
            //
            // A known account can be promoted as soon as its storage trie is drained, so an entry
            // that only awaited a storage root has to be queued again.
            if !matches!(self.pending_account_updates.insert(address, Some(account)), Some(Some(_)))
            {
                self.promotable_accounts.push(address);
            }
        }

        self.final_hashed_state.extend(hashed_state_update);
    }

    fn on_proof_result(&mut self, result: DecodedMultiProofV2) -> Result<(), StateRootTaskError> {
        self.reveal_proof_result(result)
            .map_err(|e| StateRootTaskError::Other(format!("could not reveal multiproof: {e:?}")))
    }

    /// Reveals a proof batch.
    ///
    /// The account trie is revealed here because it can never be checked out. Storage proof nodes
    /// are queued on their address' slot and revealed by the job that runs for it next.
    fn reveal_proof_result(&mut self, result: DecodedMultiProofV2) -> SparseStateTrieResult<()> {
        let DecodedMultiProofV2 { account_proofs, storage_proofs } = result;

        self.queue_storage_proofs(storage_proofs);
        // Get the workers going before spending this thread on the account trie.
        self.run_ready_storage_work()?;

        self.trie.reveal_account_proof_nodes(account_proofs)
    }

    /// Queues storage proof nodes on the slot of the address they belong to.
    fn queue_storage_proofs(&mut self, storage_proofs: B256Map<Vec<ProofTrieNodeV2>>) {
        let mut revealed_nodes = 0;
        for (address, mut nodes) in storage_proofs {
            if nodes.is_empty() {
                continue;
            }
            revealed_nodes += nodes.len();

            match self.storage_slot_mut(address) {
                StorageSlot::Idle(work) => work.queue_proofs(&mut nodes),
                StorageSlot::InFlight(in_flight) => in_flight.proofs.append(&mut nodes),
            }
        }
        self.trie.record_revealed_storage_nodes(revealed_nodes);
    }

    /// Returns the slot for `address`, checking its storage trie out of the state trie the first
    /// time the address is touched by this block.
    fn storage_slot_mut(&mut self, address: B256) -> &mut StorageSlot<S> {
        let Self { storage, trie, .. } = self;
        storage.entry(address).or_insert_with(|| {
            StorageSlot::Idle(Box::new(StorageTrieWork::new(
                trie.take_or_create_storage_trie(&address),
            )))
        })
    }

    fn on_proof_result_message(
        &mut self,
        message: ProofResultMessage,
    ) -> Result<DecodedMultiProofV2, StateRootTaskError> {
        let result = message.result?;
        debug_assert!(
            self.in_flight_proof_batches > 0,
            "received proof result without an in-flight proof batch"
        );
        self.in_flight_proof_batches = self.in_flight_proof_batches.saturating_sub(1);
        Ok(result)
    }

    fn process_new_updates(&mut self) -> SparseTrieResult<()> {
        if self.pending_updates == 0 {
            return Ok(());
        }

        let _span = debug_span!("process_new_updates").entered();
        self.pending_updates = 0;
        self.initial_updates_applied = true;

        // Queue the new storage updates on their slots; the jobs apply them to the tries.
        let Self { storage, trie, new_storage_updates, .. } = self;
        for (address, new) in new_storage_updates.drain() {
            if new.is_empty() {
                continue;
            }

            let slot = storage.entry(address).or_insert_with(|| {
                StorageSlot::Idle(Box::new(StorageTrieWork::new(
                    trie.take_or_create_storage_trie(&address),
                )))
            });
            match slot {
                StorageSlot::Idle(work) => work.queue_updates(new),
                StorageSlot::InFlight(in_flight) => merge_leaf_updates(&mut in_flight.updates, new),
            }
        }

        // Apply the new account updates to the accounts trie, which is never checked out.
        self.process_account_leaf_updates(true)?;

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

    /// Runs a pass over every idle storage slot that has something to do: proof nodes to reveal,
    /// leaf updates that were not tried against the current state of the trie yet, or a root to
    /// recompute.
    ///
    /// Each pass owns its address' payload for its whole duration, so the passes are independent
    /// of each other and of this thread. Most of them are handed to the shard pool; the ones that
    /// would cost more in handoff than in work stay here, see [`StorageTrieWork::is_target_only`]
    /// and [`INLINE_STORAGE_WORK_UNITS`].
    #[instrument(
        level = "trace",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn run_ready_storage_work(&mut self) -> SparseTrieResult<()> {
        let mut ready = Vec::new();
        let mut job_units = 0;
        for (address, slot) in &self.storage {
            let StorageSlot::Idle(work) = slot else { continue };
            if !work.has_work() {
                continue;
            }
            if !work.is_target_only() {
                job_units += work.work_units();
            }
            ready.push(*address);
        }
        if ready.is_empty() {
            return Ok(())
        }

        let inline_round = job_units <= INLINE_STORAGE_WORK_UNITS;
        let new_epoch = self.new_epoch;
        let retain_updates = self.trie.retains_updates();
        let started = Instant::now();
        let mut jobs = Vec::new();
        for address in ready {
            let slot = self.storage.get_mut(&address).expect("slot was just seen");
            let StorageSlot::Idle(work) = slot else {
                unreachable!("an idle slot is only checked out from here")
            };

            if inline_round || work.is_target_only() {
                let output = work.run(new_epoch, retain_updates);
                self.apply_storage_output(address, output)?;
                continue;
            }

            let StorageSlot::Idle(work) = core::mem::replace(
                slot,
                StorageSlot::InFlight(Box::new(InFlightStorage::new(started))),
            ) else {
                unreachable!("just matched")
            };
            self.storage_in_flight += 1;
            jobs.push(StorageTrieJob { address, work });
        }

        self.submit_storage_jobs(jobs);

        Ok(())
    }

    /// Records what a storage pass produced: the proof targets it discovered and its cache
    /// counters. Returns the error the pass stopped at, if any.
    fn apply_storage_output(
        &mut self,
        address: B256,
        output: StorageWorkOutput,
    ) -> SparseTrieResult<()> {
        let StorageWorkOutput { targets, cache_hits, cache_misses, result } = output;

        self.storage_cache_hits += cache_hits;
        self.storage_cache_misses += cache_misses;
        if !targets.is_empty() {
            self.pending_targets.extend_storage_targets(&address, targets);
        }
        if self.storage.get(&address).is_some_and(|slot| slot.is_drained()) {
            // The storage root is final now, so the account waiting for it can be promoted.
            self.promotable_accounts.push(address);
        }

        result
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

        let pending_before = account_updates.len() + self.trie.trie_mut().blocked_updates().len();

        self.trie.trie_mut().update_leaves_with_events(
            account_updates,
            true,
            |event| match event {
                LeafUpdateEvent::ProofRequired { key: target, parent } => {
                    let rounds = &mut self.account_proof_rounds;
                    match self.fetched_account_targets.entry(target) {
                        Entry::Occupied(mut entry) => {
                            let fetched = entry.get_mut();
                            if parent < fetched.parent {
                                rounds.repeat_target(fetched.rounds);
                                fetched.parent = parent;
                                fetched.rounds = fetched.rounds.saturating_add(1);
                                self.pending_targets.push_account_target(
                                    ProofV2Target::new(target).with_parent(parent),
                                );
                            } else {
                                rounds.dropped_target(parent, fetched.parent);
                            }
                        }
                        Entry::Vacant(entry) => {
                            entry.insert(FetchedTarget::new(parent));
                            rounds.first_target(parent);
                            self.pending_targets.push_account_target(
                                ProofV2Target::new(target).with_parent(parent),
                            );
                        }
                    }
                }
                LeafUpdateEvent::Touched { key, value } => {
                    if self.pending_account_updates.contains_key(&key) {
                        // The account's own leaf update is applied, so nothing keeps it from
                        // being promoted any more.
                        self.promotable_accounts.push(key);
                        if let Entry::Vacant(entry) = self.existing_accounts.entry(key) {
                            entry.insert(
                                value.filter(|value| !value.is_empty()).map(decode_trie_account),
                            );
                        }
                    }
                }
            },
        )?;

        let pending_after = account_updates.len() + self.trie.trie_mut().blocked_updates().len();
        self.account_cache_hits += pending_before.saturating_sub(pending_after) as u64;
        self.account_cache_misses += pending_after as u64;

        Ok(pending_after < pending_before)
    }

    /// Hands checked out storage tries to the shard that owns their address.
    ///
    /// A round costs one queue push per shard that has work, instead of one spawn per chunk of
    /// tries, and the shard hands its results back in batches instead of waking this thread once
    /// per trie. Because the shard of an address never changes, the pass for a storage trie runs
    /// on the thread that ran its previous pass.
    fn submit_storage_jobs(&self, jobs: Vec<StorageTrieJob<S>>) {
        if jobs.is_empty() {
            return;
        }

        let pool = storage_shard_pool();
        let parent_span = debug_span!("submit_storage_jobs", n = jobs.len());
        let mut by_shard = (0..pool.len()).map(|_| Vec::new()).collect::<Vec<_>>();
        for job in jobs {
            by_shard[pool.shard_of(&job.address)].push(job);
        }

        let new_epoch = self.new_epoch;
        let retain_updates = self.trie.retains_updates();
        for (shard, group) in by_shard.into_iter().enumerate() {
            if group.is_empty() {
                continue;
            }

            let storage_done_tx = self.storage_done_tx.clone();
            let parent_span = parent_span.clone();
            pool.submit(shard, move || {
                let _enter = debug_span!(
                    target: "engine::tree::payload_processor::sparse_trie",
                    parent: &parent_span,
                    "storage_jobs",
                    n = group.len(),
                )
                .entered();
                run_storage_jobs(group, &storage_done_tx, new_epoch, retain_updates);
            });
        }
    }

    /// Handles a message from a shard: takes a batch of finished payloads back, or resumes a
    /// panic that happened on the shard thread here, where it fails this task like an inline
    /// panic would.
    ///
    /// A pass that failed does not stop the rest of the batch from being taken back, so the
    /// error the task reports is the first one and no payload is left with the shard.
    fn on_storage_job_message(&mut self, message: StorageJobMessage<S>) -> SparseTrieResult<()> {
        match message {
            StorageJobMessage::Done(batch) => {
                self.metrics.sparse_trie_storage_return_batch_size.record(batch.len() as f64);
                let mut result = Ok(());
                for done in batch {
                    let returned = self.on_storage_trie_returned(done);
                    if result.is_ok() {
                        result = returned;
                    }
                }
                result
            }
            StorageJobMessage::Panicked { address, payload } => {
                if self.storage.remove(&address).is_some() {
                    self.storage_in_flight -= 1;
                }
                panic::resume_unwind(payload)
            }
        }
    }

    /// Puts a payload back into its slot once its job finished, folding in everything that
    /// arrived for the address while it was gone.
    fn on_storage_trie_returned(&mut self, done: StorageTrieJobDone<S>) -> SparseTrieResult<()> {
        let StorageTrieJobDone { address, work, output } = done;

        let Entry::Occupied(mut entry) = self.storage.entry(address) else {
            unreachable!("a returned payload was checked out of its slot")
        };
        let StorageSlot::InFlight(in_flight) = entry.insert(StorageSlot::Idle(work)) else {
            unreachable!("a checked out address stays in flight until its job returns")
        };
        let started = in_flight.started;
        let StorageSlot::Idle(work) = entry.into_mut() else { unreachable!("just inserted") };
        work.take_buffered(*in_flight);
        self.storage_in_flight -= 1;
        self.metrics.sparse_trie_storage_job_duration_histogram.record(started.elapsed());

        self.apply_storage_output(address, output)
    }

    /// Takes back every payload whose job has already finished, without blocking.
    fn drain_returned_storage_tries(&mut self) -> SparseTrieResult<()> {
        while let Ok(message) = self.storage_done_rx.try_recv() {
            self.on_storage_job_message(message)?;
        }

        Ok(())
    }

    /// Puts every storage trie back into the sparse state trie.
    ///
    /// The final root and everything after it - trie updates, preservation, pruning - reads the
    /// storage tries through [`SparseStateTrie`], so no address may be left in a slot.
    fn return_storage_tries(&mut self) {
        let Self { storage, trie, storage_proof_rounds, .. } = self;
        for (address, slot) in storage.drain() {
            // A payload still out with a job is only reachable on the cancellation path, where
            // the job drops it.
            if let StorageSlot::Idle(work) = slot {
                storage_proof_rounds.merge(&work.rounds);
                trie.insert_storage_trie(address, work.trie);
            }
        }
    }

    /// Promotes the pending account updates whose storage root is already available into proper
    /// leaf updates for the accounts trie.
    ///
    /// Accounts whose trie is still checked out stay pending and are promoted by a later call,
    /// once the payload has come back.
    #[instrument(
        level = "trace",
        target = "engine::tree::payload_processor::sparse_trie",
        skip_all
    )]
    fn promote_pending_account_updates(&mut self) -> SparseTrieResult<()> {
        self.process_account_leaf_updates(false)?;

        if self.pending_account_updates.is_empty() {
            return Ok(());
        }

        self.drain_promotable_accounts()?;

        #[cfg(debug_assertions)]
        self.debug_assert_no_promotable_accounts();

        Ok(())
    }

    /// Requeues every account still awaiting promotion and promotes what it can.
    fn promote_all_pending_accounts(&mut self) -> SparseTrieResult<()> {
        self.promotable_accounts.extend(self.pending_account_updates.keys().copied());
        self.drain_promotable_accounts()
    }

    /// Promotes queued accounts until neither the queue nor the accounts trie makes progress.
    fn drain_promotable_accounts(&mut self) -> SparseTrieResult<()> {
        loop {
            let span = trace_span!("promote_updates", promoted = tracing::field::Empty).entered();
            let mut num_promoted = 0;
            while let Some(addr) = self.promotable_accounts.pop() {
                num_promoted += usize::from(self.promote_account(addr));
            }
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

    /// Turns the pending update for `addr` into an accounts trie leaf update, if its storage
    /// root and its current account fields are both known by now.
    ///
    /// Returns whether the account was promoted. An account that is not ready stays pending and
    /// is queued again by whichever transition unblocks it, including the return of its trie.
    fn promote_account(&mut self, addr: B256) -> bool {
        let Some(mut pending) = self.pending_account_updates.get(&addr).copied() else {
            return false
        };

        // The root of a trie this block touched is only readable from its slot, and only while
        // no job owns it.
        let new_epoch = self.new_epoch;
        let updated_storage_root = match self.storage.get_mut(&addr) {
            Some(StorageSlot::InFlight(_)) => return false,
            Some(StorageSlot::Idle(work)) if work.updated => {
                if !work.is_drained() {
                    // If account has pending storage updates, it is still pending.
                    return false;
                }
                Some(
                    work.trie
                        .root(new_epoch)
                        .expect("updates are drained, storage trie should be revealed by now"),
                )
            }
            _ => None,
        };

        if let Some(storage_root) = updated_storage_root &&
            let Some(account) = pending.take()
        {
            self.write_account_leaf(addr, account, storage_root);
            return true;
        }

        // Get the current account state either from the trie or from latest account
        // update, which the accounts trie may be holding on to until it is revealed.
        let pending_update = match self.account_updates.get(&addr) {
            Some(update) => Some(update),
            None => self.trie.state_trie_ref().and_then(|trie| trie.blocked_updates().get(&addr)),
        };
        let trie_account = match pending_update {
            Some(LeafUpdate::Changed(encoded)) => Some(encoded)
                .filter(|encoded| !encoded.is_empty())
                .map(|encoded| decode_trie_account(encoded)),
            // Needs to be revealed first
            Some(LeafUpdate::Touched) => return false,
            // The trie reports the value of every touched leaf it applies, so an account whose
            // update was already drained is normally cached here.
            None => match self.existing_accounts.get(&addr) {
                Some(reported) => *reported,
                None => {
                    self.account_value_fallbacks += 1;
                    self.trie.get_account_value(&addr).map(|value| decode_trie_account(value))
                }
            },
        };

        let (account, storage_root) = if let Some(account) = pending.take() {
            // If account is Some(_) here it means it didn't have any storage updates
            // and we can fetch the storage root directly from the account trie.
            //
            // If it did have storage updates, we would've had processed it above when iterating
            // over storage tries.
            (account, trie_account.map_or(EMPTY_ROOT_HASH, |account| account.storage_root))
        } else {
            let storage_root = match updated_storage_root {
                Some(storage_root) => Some(storage_root),
                // An address with no slot was not touched by this block, so its trie, if any,
                // is still in the state trie with the root it was preserved with.
                None => match self.storage.get_mut(&addr) {
                    Some(StorageSlot::Idle(work)) => work.trie.root(new_epoch),
                    Some(StorageSlot::InFlight(_)) => unreachable!("returned above"),
                    None => self.trie.storage_root(&addr, new_epoch),
                },
            };
            (
                trie_account.map(Into::into),
                storage_root.expect(
                    "account had storage updates that were applied to its trie, storage root must be revealed by now",
                ),
            )
        };

        self.write_account_leaf(addr, account, storage_root);
        true
    }

    /// Queues the promoted account leaf value for the accounts trie, records it as the trie's
    /// value for `addr` so a later promotion does not have to read it back, and clears the
    /// account's pending entry.
    fn write_account_leaf(&mut self, addr: B256, account: Option<Account>, storage_root: B256) {
        let encoded = encode_account_leaf_value(account, storage_root, &mut self.account_rlp_buf);
        self.existing_accounts.insert(
            addr,
            (!encoded.is_empty())
                .then(|| account.unwrap_or_default().into_trie_account(storage_root)),
        );
        self.account_updates.insert(addr, LeafUpdate::Changed(encoded));
        self.pending_account_updates.remove(&addr);
    }

    /// Asserts the ready queue did not miss a transition: every account left pending must still
    /// be waiting for its storage trie to drain or for its own leaf update to be applied.
    #[cfg(debug_assertions)]
    fn debug_assert_no_promotable_accounts(&self) {
        for (addr, pending) in &self.pending_account_updates {
            match self.storage.get(addr) {
                // A job owns the trie, or the trie still has leaf updates to apply.
                Some(StorageSlot::InFlight(_)) => continue,
                Some(StorageSlot::Idle(work)) if !work.is_drained() => continue,
                Some(StorageSlot::Idle(work)) => assert!(
                    !(work.updated && pending.is_some()),
                    "account {addr:?} could be promoted from its drained storage trie but was not queued",
                ),
                None => {}
            }

            assert!(
                self.account_updates
                    .get(addr)
                    .or_else(|| self
                        .trie
                        .state_trie_ref()
                        .and_then(|trie| trie.blocked_updates().get(addr)))
                    .is_some_and(LeafUpdate::is_touched),
                "account {addr:?} could be promoted but was not queued",
            );
        }
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
        self.has_pending_leaf_updates() || !self.pending_account_updates.is_empty()
    }

    /// Returns whether any leaf update is still waiting to be applied to one of the tries.
    ///
    /// While this is false no trie can change any more, so every account still waiting for
    /// promotion already has everything it needs.
    fn has_pending_leaf_updates(&self) -> bool {
        !self.account_updates.is_empty() ||
            self.account_blocked_updates().is_some_and(|blocked| !blocked.is_empty()) ||
            self.storage.values().any(|slot| slot.is_pending())
    }

    /// Returns the leaf updates the accounts trie could not apply yet, if it is revealed.
    fn account_blocked_updates(&self) -> Option<&BlockedLeafUpdates> {
        self.trie.state_trie_ref().map(SparseTrie::blocked_updates)
    }

    /// Returns whether any idle slot has a pass to run, which is progress that has not been made
    /// yet rather than a stall.
    fn has_ready_storage_work(&self) -> bool {
        self.storage.values().any(|slot| match slot {
            StorageSlot::Idle(work) => work.has_untried_work(),
            StorageSlot::InFlight(_) => false,
        })
    }

    /// Errors when pending trie updates remain but nothing can deliver them: no update
    /// messages are queued, no proof targets are queued or in flight, no proof results
    /// are waiting, and no storage pass is running or waiting to run.
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
            self.storage_in_flight == 0 &&
            !self.has_ready_storage_work() &&
            self.has_pending_sparse_trie_updates()
        {
            const MAX_STALLED_PROOF_TARGETS_TO_LOG: usize = 5;

            let mut account_targets = self
                .account_updates
                .keys()
                .copied()
                .chain(self.account_blocked_updates().into_iter().flat_map(|b| b.keys()))
                .map(|target| (target, self.fetched_account_targets.get(&target).map(|f| f.parent)))
                .collect::<Vec<_>>();
            account_targets.sort_unstable();
            let account_targets_truncated =
                account_targets.len().saturating_sub(MAX_STALLED_PROOF_TARGETS_TO_LOG);
            account_targets.truncate(MAX_STALLED_PROOF_TARGETS_TO_LOG);

            let mut storage_targets = self
                .storage
                .iter()
                .filter_map(|(address, slot)| match slot {
                    StorageSlot::Idle(work) => Some((address, work)),
                    StorageSlot::InFlight(_) => None,
                })
                .flat_map(|(address, work)| {
                    work.pending.keys().copied().chain(work.trie.blocked_updates().keys()).map(
                        move |target| {
                            (*address, target, work.fetched.get(&target).map(|f| f.parent))
                        },
                    )
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

            return Err(StateRootTaskError::Stalled)
        }

        Ok(())
    }

    /// Reports how many proof round trips this block's leaf keys needed.
    fn record_proof_rounds(&mut self) {
        let account = &self.account_proof_rounds;
        let storage = &self.storage_proof_rounds;

        self.metrics.sparse_trie_account_proof_keys.record(account.keys() as f64);
        self.metrics.sparse_trie_account_multi_round_keys.record(account.multi_round_keys() as f64);
        self.metrics
            .sparse_trie_account_deeper_proof_asks
            .record(account.dropped_deeper_parent as f64);
        self.metrics.sparse_trie_storage_proof_keys.record(storage.keys() as f64);
        self.metrics.sparse_trie_storage_multi_round_keys.record(storage.multi_round_keys() as f64);
        self.metrics
            .sparse_trie_storage_deeper_proof_asks
            .record(storage.dropped_deeper_parent as f64);

        debug!(
            target: "engine::root",
            ?account,
            ?storage,
            "Proof rounds per leaf",
        );

        self.account_proof_rounds = Default::default();
        self.storage_proof_rounds = Default::default();
    }
}

/// State of one address' storage trie in the sparse trie task.
enum StorageSlot<S> {
    /// Nothing is running for this address, so its payload can be read and handed to a job.
    Idle(Box<StorageTrieWork<S>>),
    /// A job owns the payload. Everything arriving for the address meanwhile is buffered here
    /// and folded back in when the job returns.
    InFlight(Box<InFlightStorage>),
}

impl<S: SparseTrie + Default> StorageSlot<S> {
    /// Returns whether the address still owes the state root some work.
    fn is_pending(&self) -> bool {
        match self {
            Self::InFlight(_) => true,
            Self::Idle(work) => !work.is_drained() || work.has_work(),
        }
    }

    /// Returns whether every leaf update of the address was applied to its trie, which is what
    /// the account waiting for its storage root waits for.
    fn is_drained(&self) -> bool {
        match self {
            Self::InFlight(_) => false,
            Self::Idle(work) => work.is_drained(),
        }
    }
}

/// Everything the sparse trie task knows about one storage trie, moved into a job as a whole and
/// handed back by it.
struct StorageTrieWork<S> {
    /// The storage trie, checked out of the [`SparseStateTrie`] for the whole run.
    trie: RevealableSparseTrie<S>,
    /// Leaf updates that were not passed to `trie` yet. Updates the trie could not apply are
    /// owned by the trie itself, see [`SparseTrie::blocked_updates`].
    pending: B256Map<LeafUpdate>,
    /// Proof nodes to reveal into `trie` before the next leaf pass.
    proofs: Vec<ProofTrieNodeV2>,
    /// Slots whose proof was already requested, mapped to the broadest requested parent context
    /// (an unknown parent sorts before every known parent).
    fetched: B256Map<FetchedTarget>,
    /// How many proof round trips the slots of this trie needed.
    rounds: ProofRounds,
    /// Whether a pass could act on something: proof nodes to reveal, or leaf updates that were
    /// not tried against the current state of the trie yet.
    ///
    /// Blocked updates alone are not enough. Retrying them without a reveal in between cannot
    /// apply anything and would only resort the whole set again, and only this trie's own proof
    /// nodes can unblock them.
    dirty: bool,
    /// Whether this address ever had storage leaf updates queued, which is what makes promotion
    /// of its account wait for a recomputed storage root.
    updated: bool,
}

impl<S: SparseTrie + Default> StorageTrieWork<S> {
    fn new(trie: RevealableSparseTrie<S>) -> Self {
        Self {
            trie,
            pending: Default::default(),
            proofs: Vec::new(),
            fetched: Default::default(),
            rounds: Default::default(),
            dirty: false,
            updated: false,
        }
    }

    /// Reveals the queued proof nodes, applies the leaf updates that are not blocked by a blinded
    /// node, and recomputes the root once nothing is left to apply.
    fn run(&mut self, new_epoch: TrieNodeEpoch, retain_updates: bool) -> StorageWorkOutput {
        let Self { trie, pending, proofs, fetched, rounds, dirty, .. } = self;
        *dirty = false;
        let mut output = StorageWorkOutput::default();

        if !proofs.is_empty() {
            output.result = trie.reveal_v2_proof_nodes(proofs, retain_updates);
            proofs.clear();
            if output.result.is_err() {
                return output
            }
        }

        // A reveal above can have made blocked updates applicable again, which `update_leaves`
        // retries on its own even when nothing new is queued.
        if !pending.is_empty() || trie.blocked_updates().has_retryable() {
            let pending_before = pending.len() + trie.blocked_updates().len();
            let targets = &mut output.targets;
            output.result = trie.update_leaves(pending, |path, parent| match fetched.entry(path) {
                Entry::Occupied(mut entry) => {
                    let fetched = entry.get_mut();
                    if parent < fetched.parent {
                        rounds.repeat_target(fetched.rounds);
                        fetched.parent = parent;
                        fetched.rounds = fetched.rounds.saturating_add(1);
                        targets.push(ProofV2Target::new(path).with_parent(parent));
                    } else {
                        rounds.dropped_target(parent, fetched.parent);
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(FetchedTarget::new(parent));
                    rounds.first_target(parent);
                    targets.push(ProofV2Target::new(path).with_parent(parent));
                }
            });
            let pending_after = pending.len() + trie.blocked_updates().len();
            output.cache_hits = pending_before.saturating_sub(pending_after) as u64;
            output.cache_misses = pending_after as u64;
            if output.result.is_err() {
                return output
            }
        }

        if self.needs_root() {
            self.trie.root(new_epoch);
        }

        output
    }

    /// Queues leaf updates, letting a newly known value win over one that is still queued.
    fn queue_updates(&mut self, updates: B256Map<LeafUpdate>) {
        if updates.is_empty() {
            return
        }

        self.updated = true;
        self.dirty = true;
        merge_leaf_updates(&mut self.pending, updates);
    }

    /// Queues proof nodes, draining them out of the batch they arrived in.
    fn queue_proofs(&mut self, nodes: &mut Vec<ProofTrieNodeV2>) {
        if nodes.is_empty() {
            return
        }

        self.dirty = true;
        self.proofs.append(nodes);
    }

    /// Folds what arrived while a job owned this payload back into it.
    fn take_buffered(&mut self, buffered: InFlightStorage) {
        let InFlightStorage { mut proofs, updates, .. } = buffered;
        self.queue_proofs(&mut proofs);
        self.queue_updates(updates);
    }

    /// Returns whether a pass would do anything.
    fn has_work(&self) -> bool {
        self.dirty || self.trie.blocked_updates().has_retryable() || self.needs_root()
    }

    /// Returns whether a pass would try something the last pass did not already try.
    ///
    /// Every pass retries the blocked updates that wait on something other than their own path,
    /// so a slot that has only those left is not work that is still outstanding: only a reveal
    /// or a new update can change its outcome, and both set `dirty`.
    fn has_untried_work(&self) -> bool {
        self.dirty || self.needs_root()
    }

    /// Returns whether every leaf update of this address was applied to the trie.
    fn is_drained(&self) -> bool {
        self.pending.is_empty() && self.trie.blocked_updates().is_empty()
    }

    /// Returns whether the trie was modified since its root was last computed.
    ///
    /// Only a trie this block changed is hashed here: for the others the root the account leaf
    /// already carries is still correct.
    fn needs_root(&self) -> bool {
        self.updated && self.is_drained() && self.trie.is_revealed() && !self.trie.is_root_cached()
    }

    /// Returns whether a pass would only turn pending updates into proof targets.
    ///
    /// A blind trie has nothing to reveal into and nothing to apply, so running the pass on the
    /// task itself keeps the proof request from waiting for a job round trip.
    const fn is_target_only(&self) -> bool {
        self.proofs.is_empty() && self.trie.is_blind()
    }

    /// Rough cost of the next pass, for deciding whether it is worth a handoff.
    fn work_units(&self) -> usize {
        // Hashing walks everything the applied updates dirtied, which is the work the handoff
        // exists for.
        if self.needs_root() {
            return INLINE_STORAGE_WORK_UNITS + 1
        }

        // A retry rebuilds and resorts the whole blocked set, not only the entries a reveal
        // unblocked, so its cost scales with that set.
        let blocked = self.trie.blocked_updates();
        let retry = if blocked.has_retryable() { blocked.len() } else { 0 };
        self.proofs.len() + self.pending.len() + retry
    }
}

/// Buffers for one address while a job owns its payload.
struct InFlightStorage {
    /// When the payload was handed off, for the round trip histogram.
    started: Instant,
    /// Proof nodes that arrived for this address while the payload was gone.
    proofs: Vec<ProofTrieNodeV2>,
    /// Leaf updates that were queued for this address while the payload was gone.
    updates: B256Map<LeafUpdate>,
}

impl InFlightStorage {
    fn new(started: Instant) -> Self {
        Self { started, proofs: Vec::new(), updates: Default::default() }
    }
}

/// A storage payload checked out of its slot and handed to a job.
struct StorageTrieJob<S> {
    /// Hashed address the payload belongs to.
    address: B256,
    /// The checked out payload.
    work: Box<StorageTrieWork<S>>,
}

impl<S: SparseTrie + Default> StorageTrieJob<S> {
    fn run(mut self, new_epoch: TrieNodeEpoch, retain_updates: bool) -> StorageTrieJobDone<S> {
        let output = self.work.run(new_epoch, retain_updates);
        StorageTrieJobDone { address: self.address, work: self.work, output }
    }
}

/// What a shard sends back to the sparse trie task.
enum StorageJobMessage<S> {
    /// The passes of these payloads finished and the shard hands them back.
    Done(Vec<StorageTrieJobDone<S>>),
    /// A pass panicked and its trie is lost. A panic escaping a shard thread would take the
    /// thread down and wedge every later block that maps to it, so it is caught and resumed on
    /// the task thread instead, where it fails the state root calculation like an inline panic.
    Panicked {
        /// Hashed address of the payload the job owned.
        address: B256,
        /// The panic payload, resumed on the task thread.
        payload: Box<dyn Any + Send>,
    },
}

/// A storage payload coming back to the sparse trie task after its job finished.
struct StorageTrieJobDone<S> {
    /// Hashed address the payload belongs to.
    address: B256,
    /// The payload itself, to be put back into its slot.
    work: Box<StorageTrieWork<S>>,
    /// What the pass produced.
    output: StorageWorkOutput,
}

/// What one storage pass produced for the sparse trie task.
struct StorageWorkOutput {
    /// Proof targets discovered while applying the leaf updates.
    targets: Vec<ProofV2Target>,
    /// Leaf updates applied without needing a new proof.
    cache_hits: u64,
    /// Leaf updates that stayed blocked on a blinded node.
    cache_misses: u64,
    /// The error the pass stopped at, if any.
    result: SparseTrieResult<()>,
}

impl Default for StorageWorkOutput {
    fn default() -> Self {
        Self { targets: Vec::new(), cache_hits: 0, cache_misses: 0, result: Ok(()) }
    }
}

/// Runs the passes of one shard's group and hands the payloads back in growing batches.
///
/// The first payload goes back on its own, so the proof targets its pass discovered are
/// dispatched without waiting for the rest of the group, and the batch then doubles up to
/// [`MAX_STORAGE_RETURN_BATCH`]: a long queue costs the task a handful of wakeups instead of one
/// per trie, and the tries behind a batch boundary are still promoted while the shard works on
/// the ones after it. Starting at one also bounds what a group queued for a task that has since
/// been cancelled costs: it notices on the first send and drops the rest.
fn run_storage_jobs<S: SparseTrie + Default>(
    group: Vec<StorageTrieJob<S>>,
    storage_done_tx: &CrossbeamSender<StorageJobMessage<S>>,
    new_epoch: TrieNodeEpoch,
    retain_updates: bool,
) {
    let mut batch_len = 1;
    let mut done = Vec::with_capacity(batch_len);
    for job in group {
        let address = job.address;
        match panic::catch_unwind(AssertUnwindSafe(|| job.run(new_epoch, retain_updates))) {
            Ok(finished) => done.push(finished),
            Err(payload) => {
                // The task fails on the panic, but the payloads whose passes did finish are
                // still handed back so its bookkeeping stays consistent while it unwinds.
                if !done.is_empty() {
                    let _ = storage_done_tx.send(StorageJobMessage::Done(done));
                }
                let _ = storage_done_tx.send(StorageJobMessage::Panicked { address, payload });
                return;
            }
        }

        if done.len() >= batch_len {
            batch_len = (batch_len * 2).min(MAX_STORAGE_RETURN_BATCH);
            let batch = core::mem::replace(&mut done, Vec::with_capacity(batch_len));
            if storage_done_tx.send(StorageJobMessage::Done(batch)).is_err() {
                // Nobody is waiting for the result anymore, drop the rest here.
                return;
            }
        }
    }

    if !done.is_empty() {
        let _ = storage_done_tx.send(StorageJobMessage::Done(done));
    }
}

/// Merges leaf updates into a queue, letting a newly known value win over a queued one.
fn merge_leaf_updates(queued: &mut B256Map<LeafUpdate>, updates: B256Map<LeafUpdate>) {
    if queued.is_empty() {
        // Take the whole map at once, no per-slot loop.
        *queued = updates;
        return
    }

    for (slot, update) in updates {
        match queued.entry(slot) {
            Entry::Occupied(mut entry) => {
                if update.is_changed() {
                    entry.insert(update);
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(update);
            }
        }
    }
}

/// A small pool of long lived threads that run storage trie passes, one queue per shard.
///
/// The sparse trie task used to hand every pass to the global rayon pool, which meant a spawn per
/// chunk of tries, a wakeup per finished trie and CPU shared with everything else execution runs
/// there. A shard instead owns a queue and a thread: a round is one push per shard, results come
/// back in batches, and because a hashed address always maps to the same shard, the pass for a
/// storage trie runs on the thread that ran its previous pass and finds its arena in that core's
/// caches.
///
/// The pool is process wide. A sparse trie task exists for one block, its shards do not, so a
/// block pays neither thread creation nor a cold cache for a trie the previous block touched.
///
/// A shard thread is not a rayon worker, so a trie big enough to cross the arena's parallelism
/// thresholds still fans its subtries out to the global pool and blocks the shard until they are
/// done. That is the intended split: the many small tries never touch rayon at all, and the few
/// that are worth splitting keep the parallelism they had.
struct StorageShardPool {
    /// Work queue of each shard, indexed by shard.
    shards: Vec<CrossbeamSender<ShardJob>>,
}

impl StorageShardPool {
    /// Starts `shards` shard threads.
    fn new(shards: usize) -> Self {
        let shards = (0..shards)
            .map(|shard| {
                let (job_tx, job_rx) = crossbeam_channel::unbounded::<ShardJob>();
                reth_tasks::spawn_os_thread(&format!("sparse-shard-{shard:02}"), move || {
                    while let Ok(job) = job_rx.recv() {
                        // A panicking pass is caught by the job itself and resumed on the sparse
                        // trie task. This is only the net that keeps a panic anywhere else in a
                        // job from taking the shard down and wedging every later block whose
                        // addresses map to it.
                        let _ = panic::catch_unwind(AssertUnwindSafe(job));
                    }
                });
                job_tx
            })
            .collect();

        Self { shards }
    }

    /// Returns the number of shards.
    fn len(&self) -> usize {
        self.shards.len()
    }

    /// Returns the shard that owns `address`.
    fn shard_of(&self, address: &B256) -> usize {
        usize::from(address[0]) % self.shards.len()
    }

    /// Queues a job on `shard`, to run after everything already queued there.
    fn submit(&self, shard: usize, job: impl FnOnce() + Send + 'static) {
        // The shard threads only exit when the pool is dropped, which never happens.
        let _ = self.shards[shard].send(Box::new(job));
    }
}

/// A unit of work handed to a shard thread.
///
/// Erased because the pool outlives every sparse trie task and cannot be generic over the trie
/// type a task was built with.
type ShardJob = Box<dyn FnOnce() + Send>;

/// Returns the process wide storage shard pool, starting its threads on first use.
fn storage_shard_pool() -> &'static StorageShardPool {
    static POOL: OnceLock<StorageShardPool> = OnceLock::new();

    POOL.get_or_init(|| StorageShardPool::new(storage_shard_count()))
}

/// Number of shard threads to run storage trie passes on.
///
/// A quarter of the machine, at least two. The passes compete with the proof workers, the engine
/// thread and the sparse trie task itself, all of which are resident while a block is validated,
/// and the tail of a block is a long sequence of small rounds in which the handoff costs more
/// than the pass. Widening the pool would add runnable threads without shortening those rounds.
fn storage_shard_count() -> usize {
    /// Upper bound, so a large machine does not spend threads on shards that stay idle.
    const MAX_SHARDS: usize = 8;

    std::thread::available_parallelism()
        .map_or(2, |threads| threads.get())
        .div_ceil(4)
        .clamp(2, MAX_SHARDS)
}

/// Largest number of finished payloads a shard hands back in one message.
const MAX_STORAGE_RETURN_BATCH: usize = 32;

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
    /// Histogram of durations storage tries spend checked out for a job on another thread.
    pub(super) sparse_trie_storage_job_duration_histogram: Histogram,
    /// Histogram of how many storage tries a shard hands back in one message, which is how many
    /// tries the task takes back per wakeup.
    pub(super) sparse_trie_storage_return_batch_size: Histogram,
    /// Histogram of sparse trie final update durations.
    pub(super) sparse_trie_final_update_duration_histogram: Histogram,
    /// Histogram of sparse trie total durations.
    pub(super) sparse_trie_total_duration_histogram: Histogram,
    /// Time spent preparing the sparse trie for reuse after state root computation.
    pub(super) into_trie_for_reuse_duration_histogram: Histogram,
    /// Time spent pruning the sparse trie by node epoch.
    pub(super) sparse_trie_prune_duration_histogram: Histogram,
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
    /// Number of account values promotion had to read back from the accounts trie because the
    /// trie never reported them while applying a touched update.
    pub(super) sparse_trie_account_value_fallbacks: Histogram,

    /// Number of account keys a proof target was dispatched for.
    pub(super) sparse_trie_account_proof_keys: Histogram,
    /// Number of account keys that needed more than one proof target.
    pub(super) sparse_trie_account_multi_round_keys: Histogram,
    /// Number of times the accounts trie asked for a proof below an already requested parent,
    /// which no target is dispatched for.
    pub(super) sparse_trie_account_deeper_proof_asks: Histogram,
    /// Number of storage keys a proof target was dispatched for.
    pub(super) sparse_trie_storage_proof_keys: Histogram,
    /// Number of storage keys that needed more than one proof target.
    pub(super) sparse_trie_storage_multi_round_keys: Histogram,
    /// Number of times a storage trie asked for a proof below an already requested parent,
    /// which no target is dispatched for.
    pub(super) sparse_trie_storage_deeper_proof_asks: Histogram,

    /// Number of storage tries retained in the preserved sparse trie cache.
    pub(super) sparse_trie_retained_storage_tries: Gauge,
}

/// The default max targets, for limiting the number of account and storage proof targets to be
/// fetched by a single worker. If exceeded, chunking is forced regardless of worker availability.
const DEFAULT_MAX_TARGETS_FOR_CHUNKING: usize = 300;

/// Start proof fetching while the first state-update batch is still arriving.
const INITIAL_UPDATE_BATCH_SIZE: usize = 64;

/// A round of storage passes that would do at most this much work - proof nodes to reveal plus
/// leaf updates to apply - runs on the sparse trie task itself, because handing the tries to
/// another thread and waiting for them to come back costs more than the work.
const INLINE_STORAGE_WORK_UNITS: usize = 16;

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

/// Decodes an account trie leaf value.
fn decode_trie_account(encoded: &[u8]) -> TrieAccount {
    TrieAccount::decode(&mut &encoded[..]).expect("invalid account RLP")
}

/// RLP-encodes the account as a [`TrieAccount`] leaf value, or returns empty for deletions.
///
/// `Some(Account::default())` with an empty storage root is encoded as a deletion. This is valid
/// for post-Merge state because EIP-7523 (<https://eips.ethereum.org/EIPS/eip-7523>) prohibits
/// empty accounts. Do not use this encoding rule when replaying historical pre-Merge state, where
/// an empty account and a missing account can have different trie representations.
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

/// What one trie already asked the proof workers for one leaf key.
#[derive(Debug, Clone, Copy)]
struct FetchedTarget {
    /// Broadest parent context requested so far. An unknown parent sorts before every known
    /// parent, so this only ever moves towards the trie root.
    parent: ProofV2TargetParent,
    /// Proof targets dispatched for this key, saturating.
    rounds: u8,
}

impl FetchedTarget {
    /// Records the first dispatched target for a key.
    const fn new(parent: ProofV2TargetParent) -> Self {
        Self { parent, rounds: 1 }
    }
}

/// Number of round buckets in [`ProofRounds`]; the last one absorbs everything above it.
const PROOF_ROUND_BUCKETS: usize = 4;

/// Number of parent depth buckets in [`ProofRounds`]; the last one absorbs everything deeper.
const PARENT_DEPTH_BUCKETS: usize = 12;

/// How many proof round trips the leaf keys of one or more tries needed in this block.
///
/// A leaf update that hits a blinded node makes the trie ask for a proof for its key. The ask is
/// only turned into a target when no proof for that key was requested from an equally broad or
/// broader parent yet, so a key needing more than one target means one proof did not reveal
/// enough of its path. An ask that was dropped instead is counted separately: with a strictly
/// deeper parent it is the trie finding the next blinded node further down the same path, which
/// no target would be dispatched for.
#[derive(Debug, Default, Clone, Copy)]
struct ProofRounds {
    /// Keys by dispatched target count; index `i` holds the keys with `i + 1` targets.
    keys_by_round: [u32; PROOF_ROUND_BUCKETS],
    /// Asks dropped because the key was already requested from the same parent.
    dropped_same_parent: u32,
    /// Asks dropped because the key was already requested from a shallower parent.
    dropped_deeper_parent: u32,
    /// First targets of a key that had to include the trie root.
    root_targets: u32,
    /// First targets of a key by the depth of their parent hint.
    first_targets_by_parent_depth: [u32; PARENT_DEPTH_BUCKETS],
    /// Sum of the parent hint depths counted in [`Self::first_targets_by_parent_depth`].
    parent_depth_sum: u64,
}

impl ProofRounds {
    /// Records the first target dispatched for a key.
    fn first_target(&mut self, parent: ProofV2TargetParent) {
        self.keys_by_round[0] += 1;
        match parent.path_len() {
            Some(depth) => {
                self.first_targets_by_parent_depth[depth.min(PARENT_DEPTH_BUCKETS - 1)] += 1;
                self.parent_depth_sum += depth as u64;
            }
            None => self.root_targets += 1,
        }
    }

    /// Records another target dispatched for a key that already had `rounds` of them.
    fn repeat_target(&mut self, rounds: u8) {
        let from = Self::bucket(rounds);
        let to = Self::bucket(rounds.saturating_add(1));
        if from != to {
            self.keys_by_round[from] -= 1;
            self.keys_by_round[to] += 1;
        }
    }

    /// Records an ask that was dropped in favour of the target already dispatched from `fetched`.
    fn dropped_target(&mut self, parent: ProofV2TargetParent, fetched: ProofV2TargetParent) {
        if parent > fetched {
            self.dropped_deeper_parent += 1;
        } else {
            self.dropped_same_parent += 1;
        }
    }

    /// Adds another trie's counts to these.
    fn merge(&mut self, other: &Self) {
        for (total, count) in self.keys_by_round.iter_mut().zip(other.keys_by_round) {
            *total += count;
        }
        for (total, count) in
            self.first_targets_by_parent_depth.iter_mut().zip(other.first_targets_by_parent_depth)
        {
            *total += count;
        }
        self.dropped_same_parent += other.dropped_same_parent;
        self.dropped_deeper_parent += other.dropped_deeper_parent;
        self.root_targets += other.root_targets;
        self.parent_depth_sum += other.parent_depth_sum;
    }

    /// Returns the number of keys a proof was requested for.
    fn keys(&self) -> u32 {
        self.keys_by_round.iter().sum()
    }

    /// Returns the number of keys that needed more than one dispatched target.
    fn multi_round_keys(&self) -> u32 {
        self.keys_by_round[1..].iter().sum()
    }

    /// Returns the bucket a key with the given number of dispatched targets belongs in.
    fn bucket(rounds: u8) -> usize {
        (rounds.max(1) as usize - 1).min(PROOF_ROUND_BUCKETS - 1)
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
    use reth_db_common::init::init_genesis;
    use reth_provider::test_utils::create_test_provider_factory;
    use reth_storage_overlay::{OverlayManager, OverlayStateProviderFactory};
    use reth_trie_common::{LeafNode, Nibbles, TrieNodeV2};
    use reth_trie_parallel::proof_task::ProofTaskCtx;
    use reth_trie_sparse::ArenaParallelSparseTrie;

    fn drain_sparse_trie_tasks(runtime: &Runtime) {
        for task_name in ["trie-hashing", "storage-workers", "account-workers"] {
            runtime.spawn_blocking_named(task_name, || {}).get();
        }
    }

    /// Checks a payload out of its slot the way a spawned job does, without racing one.
    fn check_out_storage(
        task: &mut SparseTrieCacheTask,
        address: B256,
    ) -> Box<StorageTrieWork<ArenaParallelSparseTrie>> {
        let slot = task.storage.get_mut(&address).expect("address must have a slot");
        let StorageSlot::Idle(work) = core::mem::replace(
            slot,
            StorageSlot::InFlight(Box::new(InFlightStorage::new(Instant::now()))),
        ) else {
            panic!("slot must be idle")
        };
        task.storage_in_flight += 1;
        work
    }

    /// Hands a payload checked out by [`check_out_storage`] back, as a finished job would.
    fn return_storage(
        task: &mut SparseTrieCacheTask,
        address: B256,
        work: Box<StorageTrieWork<ArenaParallelSparseTrie>>,
    ) {
        task.on_storage_trie_returned(StorageTrieJobDone {
            address,
            work,
            output: StorageWorkOutput::default(),
        })
        .unwrap();
    }

    fn storage_slot_value(
        task: &SparseTrieCacheTask,
        address: &B256,
        slot: &B256,
    ) -> Option<Vec<u8>> {
        let StorageSlot::Idle(work) = task.storage.get(address)? else { return None };
        work.trie.as_revealed_ref()?.get_leaf_value(&Nibbles::unpack(slot)).cloned()
    }

    fn storage_root_of(task: &mut SparseTrieCacheTask, address: B256) -> B256 {
        let StorageSlot::Idle(work) = task.storage.get_mut(&address).expect("slot") else {
            panic!("payload is out with a job")
        };
        work.trie.root(TrieNodeEpoch::new(1)).expect("storage trie must be revealed")
    }

    /// Builds a job for `address` whose pass applies one leaf update to a revealed empty trie.
    fn storage_job(address: B256) -> StorageTrieJob<ArenaParallelSparseTrie> {
        let mut work = Box::new(StorageTrieWork::new(RevealableSparseTrie::revealed_empty()));
        work.queue_updates(B256Map::from_iter([(
            B256::repeat_byte(0x77),
            LeafUpdate::Changed(alloy_rlp::encode_fixed_size(&U256::from(1)).to_vec()),
        )]));
        StorageTrieJob { address, work }
    }

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
        let mut storage = reth_trie::HashedStorage::default();
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
    fn test_encode_account_leaf_value_deletion_and_empty_root_is_empty() {
        let mut account_rlp_buf = vec![0xAB];
        let encoded = encode_account_leaf_value(None, EMPTY_ROOT_HASH, &mut account_rlp_buf);

        assert!(encoded.is_empty());
        // Early return should not touch the caller's buffer.
        assert_eq!(account_rlp_buf, vec![0xAB]);
    }

    #[test]
    fn test_encode_account_leaf_value_empty_account_and_empty_root_is_empty() {
        let mut account_rlp_buf = vec![0xAB];
        let encoded = encode_account_leaf_value(
            Some(Account::default()),
            EMPTY_ROOT_HASH,
            &mut account_rlp_buf,
        );

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
    fn checked_out_storage_trie_holds_back_its_updates_until_it_returns() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = init_genesis(&provider_factory).expect("failed to initialize genesis");
        let state_provider_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayManager::<reth_chain_state::EthPrimitives>::default()
                .overlay_builder(anchor_hash),
        );
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let proof_worker_handle = ProofWorkerHandle::new(
            &runtime,
            ProofTaskCtx::new(state_provider_factory),
            false,
            proof_result_tx.clone(),
        );

        let trie = SparseStateTrie::default()
            .with_accounts_trie(RevealableSparseTrie::<ArenaParallelSparseTrie>::revealed_empty())
            .with_default_storage_trie(RevealableSparseTrie::blind_from(
                ArenaParallelSparseTrie::default(),
            ))
            .with_updates(true);

        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (_cancel_guard, cancel_rx) = crossbeam_channel::bounded::<()>(0);
        let mut task = SparseTrieCacheTask::new_with_trie(
            &runtime,
            updates_rx,
            cancel_rx,
            std::sync::mpsc::channel().0,
            proof_worker_handle,
            proof_result_tx,
            proof_result_rx,
            SparseTrieTaskMetrics::default(),
            trie,
            EMPTY_ROOT_HASH,
            TrieNodeEpoch::new(1),
            1,
        );

        let address = B256::repeat_byte(0x11);
        let revealed_slot = B256::repeat_byte(0x22);
        let late_slot = B256::repeat_byte(0x33);
        let revealed_value = alloy_rlp::encode_fixed_size(&U256::from(7)).to_vec();
        let late_value = alloy_rlp::encode_fixed_size(&U256::from(9)).to_vec();
        let later_value = alloy_rlp::encode_fixed_size(&U256::from(11)).to_vec();

        let mut state = HashedPostState::default();
        state.accounts.insert(address, Some(Account { nonce: 1, ..Default::default() }));
        state.storages.entry(address).or_default().storage.insert(revealed_slot, U256::from(7));
        task.on_hashed_state_update(state);
        task.pending_updates = 1;
        task.process_new_updates().unwrap();

        // Check the payload out by hand so the test does not race a spawned job.
        let work = check_out_storage(&mut task, address);
        task.finished_state_updates = true;

        assert!(task.has_pending_sparse_trie_updates());
        assert!(
            task.ensure_not_stalled(false).is_ok(),
            "a checked out payload can still deliver progress"
        );

        // A proof for a checked out address must be buffered, not revealed into a fresh blind
        // trie that the returning job would then overwrite.
        let leaf = ProofTrieNodeV2 {
            path: Nibbles::default(),
            node: TrieNodeV2::Leaf(LeafNode::new(
                Nibbles::unpack(revealed_slot),
                revealed_value.clone(),
            )),
            masks: None,
        };
        task.on_proof_result(DecodedMultiProofV2 {
            storage_proofs: B256Map::from_iter([(address, vec![leaf])]),
            ..Default::default()
        })
        .unwrap();
        assert!(!task.trie.storage_tries_mut().contains_key(&address));
        let StorageSlot::InFlight(in_flight) = &task.storage[&address] else {
            panic!("payload is out with a job")
        };
        assert_eq!(in_flight.proofs.len(), 1);

        // The same holds for leaf updates arriving while the payload is gone.
        let mut state = HashedPostState::default();
        state.storages.entry(address).or_default().storage.insert(late_slot, U256::from(9));
        task.on_hashed_state_update(state);
        task.pending_updates = 1;
        task.process_new_updates().unwrap();
        let StorageSlot::InFlight(in_flight) = &task.storage[&address] else {
            panic!("payload is out with a job")
        };
        assert!(in_flight.updates.contains_key(&late_slot));

        return_storage(&mut task, address, work);
        assert_eq!(task.storage_in_flight, 0);

        // The next pass reveals what was buffered and applies both the update it was checked out
        // with and the one that arrived meanwhile.
        task.run_ready_storage_work().unwrap();
        assert_eq!(storage_slot_value(&task, &address, &revealed_slot), Some(revealed_value));
        assert_eq!(storage_slot_value(&task, &address, &late_slot), Some(late_value));

        let StorageSlot::Idle(work) = &task.storage[&address] else { panic!("payload is back") };
        assert!(work.pending.is_empty());
        assert!(work.trie.is_root_cached(), "a drained payload hashes its trie");
        let root_before = storage_root_of(&mut task, address);

        // An update arriving during a job invalidates the root the previous pass computed.
        let work = check_out_storage(&mut task, address);
        let mut state = HashedPostState::default();
        state.storages.entry(address).or_default().storage.insert(late_slot, U256::from(11));
        task.on_hashed_state_update(state);
        task.pending_updates = 1;
        task.process_new_updates().unwrap();
        return_storage(&mut task, address, work);

        let StorageSlot::Idle(work) = &task.storage[&address] else { panic!("payload is back") };
        assert!(work.has_work(), "a buffered update makes the payload ready again");

        task.run_ready_storage_work().unwrap();
        assert_eq!(storage_slot_value(&task, &address, &late_slot), Some(later_value));
        assert_ne!(storage_root_of(&mut task, address), root_before);

        drop(updates_tx);
        drop(task);
        drain_sparse_trie_tasks(&runtime);
    }

    #[test]
    fn large_storage_batches_run_off_thread() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = init_genesis(&provider_factory).expect("failed to initialize genesis");
        let state_provider_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayManager::<reth_chain_state::EthPrimitives>::default()
                .overlay_builder(anchor_hash),
        );
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let proof_worker_handle = ProofWorkerHandle::new(
            &runtime,
            ProofTaskCtx::new(state_provider_factory),
            false,
            proof_result_tx.clone(),
        );

        let trie = SparseStateTrie::default()
            .with_accounts_trie(RevealableSparseTrie::<ArenaParallelSparseTrie>::revealed_empty())
            .with_default_storage_trie(RevealableSparseTrie::blind_from(
                ArenaParallelSparseTrie::default(),
            ))
            .with_updates(true);

        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (_cancel_guard, cancel_rx) = crossbeam_channel::bounded::<()>(0);
        let mut task = SparseTrieCacheTask::new_with_trie(
            &runtime,
            updates_rx,
            cancel_rx,
            std::sync::mpsc::channel().0,
            proof_worker_handle,
            proof_result_tx,
            proof_result_rx,
            SparseTrieTaskMetrics::default(),
            trie,
            EMPTY_ROOT_HASH,
            TrieNodeEpoch::new(1),
            1,
        );

        // One leaf per address, enough of them to exceed the inline budget.
        let leaves = (0..=INLINE_STORAGE_WORK_UNITS as u8)
            .map(|index| {
                let address = B256::repeat_byte(0x10 + index);
                let slot = B256::repeat_byte(0x80 + index);
                let value = alloy_rlp::encode_fixed_size(&U256::from(index + 1)).to_vec();
                (address, slot, value)
            })
            .collect::<Vec<_>>();
        let storage_proofs = leaves
            .iter()
            .map(|(address, slot, value)| {
                let leaf = ProofTrieNodeV2 {
                    path: Nibbles::default(),
                    node: TrieNodeV2::Leaf(LeafNode::new(Nibbles::unpack(slot), value.clone())),
                    masks: None,
                };
                (*address, vec![leaf])
            })
            .collect();

        task.on_proof_result(DecodedMultiProofV2 { account_proofs: Vec::new(), storage_proofs })
            .unwrap();

        assert_eq!(task.storage_in_flight, leaves.len());
        assert!(task.has_pending_sparse_trie_updates(), "completion must wait for the reveals");

        while task.storage_in_flight > 0 {
            task.drain_returned_storage_tries().unwrap();
        }
        for (address, slot, value) in &leaves {
            assert_eq!(storage_slot_value(&task, address, slot).as_ref(), Some(value));
        }

        drop(updates_tx);
        drop(task);
        drain_sparse_trie_tasks(&runtime);
    }

    #[test]
    fn run_waits_for_storage_tries_hashed_off_thread() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = init_genesis(&provider_factory).expect("failed to initialize genesis");
        let state_provider_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayManager::<reth_chain_state::EthPrimitives>::default()
                .overlay_builder(anchor_hash),
        );
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let proof_worker_handle = ProofWorkerHandle::new(
            &runtime,
            ProofTaskCtx::new(state_provider_factory),
            false,
            proof_result_tx.clone(),
        );

        let default_trie = RevealableSparseTrie::<ArenaParallelSparseTrie>::revealed_empty();
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
            proof_result_tx,
            proof_result_rx,
            SparseTrieTaskMetrics::default(),
            trie,
            EMPTY_ROOT_HASH,
            TrieNodeEpoch::new(1),
            1,
        );

        let accounts = (0..8u8)
            .map(|index| {
                let address = B256::repeat_byte(0x10 + index);
                let account = Account {
                    nonce: u64::from(index) + 1,
                    balance: U256::from(index),
                    bytecode_hash: None,
                };
                let storage = (0..4u8)
                    .map(|slot| {
                        (
                            B256::repeat_byte(0x40 + index * 4 + slot),
                            U256::from(slot) + U256::from(1),
                        )
                    })
                    .collect::<Vec<_>>();
                (address, account, storage)
            })
            .collect::<Vec<_>>();

        let mut state = HashedPostState::default();
        for (address, account, storage) in &accounts {
            state.accounts.insert(*address, Some(*account));
            state.storages.entry(*address).or_default().storage.extend(storage.iter().copied());
        }
        updates_tx.send(StateRootMessage::HashedStateUpdate(state)).unwrap();
        updates_tx.send(StateRootMessage::FinishedStateUpdates).unwrap();

        let outcome = task.run().expect("state root computation should succeed");

        let expected = accounts.iter().map(|(address, account, storage)| {
            let storage_root =
                reth_trie_common::root::storage_root_unsorted(storage.iter().copied());
            (*address, account.into_trie_account(storage_root))
        });
        assert_eq!(outcome.state_root, reth_trie_common::root::state_root_unsorted(expected));
        assert_eq!(task.storage_in_flight, 0);
        assert!(task.storage.is_empty(), "every storage trie is back in the state trie");

        drop(updates_tx);
        drop(task);
        drain_sparse_trie_tasks(&runtime);
    }

    #[test]
    fn first_leaf_batch_starts_proofs_before_input_queue_drains() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = init_genesis(&provider_factory).expect("failed to initialize genesis");
        let state_provider_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayManager::<reth_chain_state::EthPrimitives>::default()
                .overlay_builder(anchor_hash),
        );
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let proof_worker_handle = ProofWorkerHandle::new(
            &runtime,
            ProofTaskCtx::new(state_provider_factory),
            false,
            proof_result_tx.clone(),
        );

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
            proof_result_tx,
            proof_result_rx,
            SparseTrieTaskMetrics::default(),
            trie,
            parent_state_root,
            TrieNodeEpoch::UNMODIFIED,
            1,
        );

        // Keep an input queued so progress cannot use its normal queue-empty flush.
        updates_tx.send(StateRootMessage::PrefetchProofs(Default::default())).unwrap();
        let deadline = std::time::Instant::now();
        while task.updates.is_empty() {
            assert!(deadline.elapsed() < std::time::Duration::from_secs(1));
            std::thread::yield_now();
        }
        for index in 0..INITIAL_UPDATE_BATCH_SIZE {
            let mut state = HashedPostState::default();
            state.accounts.insert(
                B256::repeat_byte(index as u8),
                Some(Account { nonce: 1, ..Default::default() }),
            );
            task.on_hashed_state_update(state);
            task.pending_updates += 1;
            assert!(!task.make_progress().unwrap());
            if index + 1 < INITIAL_UPDATE_BATCH_SIZE {
                assert_eq!(task.in_flight_proof_batches, 0);
            }
        }
        assert!(task.in_flight_proof_batches > 0, "proof work must start before the queue drains");
        assert_eq!(task.pending_updates, 0);

        // A second batch remains buffered; the early flush must not become a permanent small
        // batch policy that repeatedly scans and sorts pending leaves.
        for index in INITIAL_UPDATE_BATCH_SIZE..INITIAL_UPDATE_BATCH_SIZE * 2 {
            let mut state = HashedPostState::default();
            state.accounts.insert(
                B256::repeat_byte(index as u8),
                Some(Account { nonce: 1, ..Default::default() }),
            );
            task.on_hashed_state_update(state);
            task.pending_updates += 1;
            assert!(!task.make_progress().unwrap());
        }
        assert_eq!(task.pending_updates, INITIAL_UPDATE_BATCH_SIZE);
        assert_eq!(task.new_account_updates.len(), INITIAL_UPDATE_BATCH_SIZE);
        drop(updates_tx);
        drop(task);
        drain_sparse_trie_tasks(&runtime);
    }

    #[test]
    fn storage_leaves_retry_only_after_a_proof_for_their_trie() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = init_genesis(&provider_factory).expect("failed to initialize genesis");
        let state_provider_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayManager::<reth_chain_state::EthPrimitives>::default()
                .overlay_builder(anchor_hash),
        );
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let proof_worker_handle = ProofWorkerHandle::new(
            &runtime,
            ProofTaskCtx::new(state_provider_factory),
            false,
            proof_result_tx.clone(),
        );

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
            proof_result_tx,
            proof_result_rx,
            SparseTrieTaskMetrics::default(),
            trie,
            B256::from([0x55; 32]),
            TrieNodeEpoch::UNMODIFIED,
            1,
        );

        let first = B256::repeat_byte(0x11);
        let second = B256::repeat_byte(0x22);
        let slot = B256::repeat_byte(0x33);
        task.on_prewarm_targets(MultiProofTargetsV2 {
            storage_targets: B256Map::from_iter([
                (first, vec![ProofV2Target::new(slot)]),
                (second, vec![ProofV2Target::new(slot)]),
            ]),
            ..Default::default()
        });
        task.pending_updates = 1;
        task.process_new_updates().unwrap();
        task.run_ready_storage_work().unwrap();

        let misses = task.storage_cache_misses;
        task.run_ready_storage_work().unwrap();
        assert_eq!(task.storage_cache_misses, misses, "tries without a proof must not be visited");

        // Revealing one storage trie must drain its pending touch without visiting the other.
        task.on_proof_result(DecodedMultiProofV2 {
            storage_proofs: B256Map::from_iter([(
                first,
                vec![reth_trie_common::ProofTrieNodeV2::empty()],
            )]),
            ..Default::default()
        })
        .unwrap();
        let StorageSlot::Idle(revealed) = &task.storage[&first] else {
            panic!("the pass for a revealed trie runs on the task")
        };
        assert!(revealed.is_drained());
        let StorageSlot::Idle(untouched) = &task.storage[&second] else {
            panic!("a trie without a proof is never handed to a job")
        };
        assert_eq!(untouched.pending.len(), 1);
        assert_eq!(task.storage_cache_misses, misses);

        drop(updates_tx);
        drop(task);
        drain_sparse_trie_tasks(&runtime);
    }

    #[test]
    fn storage_only_change_promotes_from_the_reported_account_value() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = init_genesis(&provider_factory).expect("failed to initialize genesis");
        let state_provider_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayManager::<reth_chain_state::EthPrimitives>::default()
                .overlay_builder(anchor_hash),
        );
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let proof_worker_handle = ProofWorkerHandle::new(
            &runtime,
            ProofTaskCtx::new(state_provider_factory),
            false,
            proof_result_tx.clone(),
        );

        let address = B256::repeat_byte(0x11);
        let account = Account {
            nonce: 7,
            balance: U256::from(42),
            bytecode_hash: Some(B256::repeat_byte(9)),
        };

        // Seed the accounts trie with the account's parent-state leaf. Both tries start revealed
        // and empty so the promotion path runs without any proof round trips.
        let mut accounts_trie = RevealableSparseTrie::<ArenaParallelSparseTrie>::revealed_empty();
        let mut seed = B256Map::from_iter([(
            address,
            LeafUpdate::Changed(encode_account_leaf_value(
                Some(account),
                EMPTY_ROOT_HASH,
                &mut Vec::new(),
            )),
        )]);
        accounts_trie
            .update_leaves(&mut seed, |_, _| panic!("a revealed empty trie needs no proofs"))
            .unwrap();

        let trie = SparseStateTrie::default()
            .with_accounts_trie(accounts_trie)
            .with_default_storage_trie(
                RevealableSparseTrie::<ArenaParallelSparseTrie>::revealed_empty(),
            )
            .with_updates(true);

        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (_cancel_guard, cancel_rx) = crossbeam_channel::bounded::<()>(0);
        let mut task = SparseTrieCacheTask::new_with_trie(
            &runtime,
            updates_rx,
            cancel_rx,
            std::sync::mpsc::channel().0,
            proof_worker_handle,
            proof_result_tx,
            proof_result_rx,
            SparseTrieTaskMetrics::default(),
            trie,
            B256::from([0x55; 32]),
            TrieNodeEpoch::UNMODIFIED,
            1,
        );

        let mut hashed_state = HashedPostState::default();
        let mut storage = reth_trie::HashedStorage::default();
        storage.storage.insert(B256::repeat_byte(0x22), U256::from(5));
        hashed_state.storages.insert(address, storage);
        task.on_hashed_state_update(hashed_state);
        task.pending_updates = 1;
        task.process_new_updates().unwrap();
        task.run_ready_storage_work().unwrap();
        task.promote_pending_account_updates().unwrap();

        assert!(task.pending_account_updates.is_empty(), "the account should have been promoted");
        assert_eq!(
            task.account_value_fallbacks, 0,
            "the touched report must cover the promoted account"
        );

        let promoted = task.trie.get_account_value(&address).expect("account leaf was written");
        let promoted = TrieAccount::decode(&mut &promoted[..]).unwrap();
        assert_eq!(promoted.nonce, account.nonce, "unchanged fields must survive promotion");
        assert_eq!(promoted.balance, account.balance);
        assert_ne!(promoted.storage_root, EMPTY_ROOT_HASH, "the storage change must be applied");

        drop(updates_tx);
        drop(task);
        drain_sparse_trie_tasks(&runtime);
    }

    #[test]
    fn run_returns_parent_root_without_revealing_blind_trie_when_no_state_updates() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = init_genesis(&provider_factory).expect("failed to initialize genesis");
        let state_provider_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayManager::<reth_chain_state::EthPrimitives>::default()
                .overlay_builder(anchor_hash),
        );
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let proof_worker_handle = ProofWorkerHandle::new(
            &runtime,
            ProofTaskCtx::new(state_provider_factory),
            false,
            proof_result_tx.clone(),
        );

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
            proof_result_tx,
            proof_result_rx,
            SparseTrieTaskMetrics::default(),
            trie,
            parent_state_root,
            TrieNodeEpoch::UNMODIFIED,
            1,
        );

        updates_tx.send(StateRootMessage::FinishedStateUpdates).unwrap();
        drop(updates_tx);

        let outcome = task.run().expect("state root computation should succeed");

        assert_eq!(outcome.state_root, parent_state_root);
        assert!(outcome.trie_updates.is_empty());
        assert!(task.trie.state_trie_ref().is_none(), "blind trie should not be revealed");

        drop(task);
        drain_sparse_trie_tasks(&runtime);
    }

    #[test]
    fn stall_check_waits_for_in_flight_proofs_then_reports_pending_updates() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = init_genesis(&provider_factory).expect("failed to initialize genesis");
        let state_provider_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayManager::<reth_chain_state::EthPrimitives>::default()
                .overlay_builder(anchor_hash),
        );
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let proof_worker_handle = ProofWorkerHandle::new(
            &runtime,
            ProofTaskCtx::new(state_provider_factory),
            false,
            proof_result_tx.clone(),
        );

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
            proof_result_tx,
            proof_result_rx,
            SparseTrieTaskMetrics::default(),
            trie,
            B256::from([0x55; 32]),
            TrieNodeEpoch::UNMODIFIED,
            1,
        );

        drop(updates_tx);

        let account = B256::from([0x11; 32]);
        let slot = B256::from([0x22; 32]);
        let account_target = B256::from([0x33; 32]);
        let storage_target = B256::from([0x44; 32]);

        task.finished_state_updates = true;
        task.account_updates.insert(account, LeafUpdate::Touched);
        task.pending_account_updates.insert(account, None);
        task.fetched_account_targets
            .insert(account_target, FetchedTarget::new(ProofV2TargetParent::NONE));

        // A storage leaf that stayed blocked after its pass ran, so no pass is ready to run.
        let StorageSlot::Idle(work) = task.storage_slot_mut(account) else {
            panic!("a fresh slot is idle")
        };
        work.updated = true;
        work.pending.insert(slot, LeafUpdate::Touched);
        work.fetched.insert(storage_target, FetchedTarget::new(ProofV2TargetParent::new(11)));
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

        drop(task);
        drain_sparse_trie_tasks(&runtime);
    }

    #[test]
    fn run_errors_when_cancel_guard_drops_before_updates_finish() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = init_genesis(&provider_factory).expect("failed to initialize genesis");
        let state_provider_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayManager::<reth_chain_state::EthPrimitives>::default()
                .overlay_builder(anchor_hash),
        );
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let proof_worker_handle = ProofWorkerHandle::new(
            &runtime,
            ProofTaskCtx::new(state_provider_factory),
            false,
            proof_result_tx.clone(),
        );

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
            proof_result_tx,
            proof_result_rx,
            SparseTrieTaskMetrics::default(),
            trie,
            B256::from([0x55; 32]),
            TrieNodeEpoch::UNMODIFIED,
            1,
        );

        // The consumer abandons the computation. The updates channel is still open (no finish
        // marker was sent), so without the cancel signal the task would wait forever.
        drop(cancel_guard);

        let error = task.run().expect_err("canceled task must return an error");
        assert!(matches!(error, StateRootTaskError::Canceled));

        drop(updates_tx);
        drop(task);
        drain_sparse_trie_tasks(&runtime);
    }

    #[test]
    fn run_ignores_hints_queued_after_updates_finish() {
        let runtime = reth_tasks::Runtime::test();
        let provider_factory = create_test_provider_factory();
        let anchor_hash = init_genesis(&provider_factory).expect("failed to initialize genesis");
        let state_provider_factory = OverlayStateProviderFactory::new(
            provider_factory,
            OverlayManager::<reth_chain_state::EthPrimitives>::default()
                .overlay_builder(anchor_hash),
        );
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let proof_worker_handle = ProofWorkerHandle::new(
            &runtime,
            ProofTaskCtx::new(state_provider_factory),
            false,
            proof_result_tx.clone(),
        );

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
            proof_result_tx,
            proof_result_rx,
            SparseTrieTaskMetrics::default(),
            trie,
            B256::from([0x55; 32]),
            TrieNodeEpoch::UNMODIFIED,
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

        drop(updates_tx);
        drain_sparse_trie_tasks(&runtime);
    }

    #[test]
    fn addresses_of_one_shard_are_processed_in_order_on_its_thread() {
        let pool = storage_shard_pool();
        // Two addresses that share a leading byte always share a shard.
        let first = B256::repeat_byte(0x5a);
        let mut second = B256::repeat_byte(0x5a);
        second[31] = 0x01;
        let shard = pool.shard_of(&first);
        assert_eq!(shard, pool.shard_of(&second));

        let (run_tx, run_rx) = crossbeam_channel::unbounded();
        for address in [first, second] {
            let run_tx = run_tx.clone();
            pool.submit(shard, move || {
                let _ = run_tx.send((address, std::thread::current().id()));
            });
        }
        drop(run_tx);

        let (first_address, first_thread) = run_rx.recv().expect("first job must run");
        let (second_address, second_thread) = run_rx.recv().expect("second job must run");
        assert_eq!([first_address, second_address], [first, second]);
        assert_eq!(first_thread, second_thread, "a shard is one thread");
    }

    #[test]
    fn a_shard_hands_its_group_back_in_growing_batches() {
        const JOBS: usize = MAX_STORAGE_RETURN_BATCH * 2;

        let addresses = (0..JOBS).map(|index| B256::from(U256::from(index))).collect::<Vec<_>>();
        let (done_tx, done_rx) = crossbeam_channel::unbounded();
        run_storage_jobs(
            addresses.iter().copied().map(storage_job).collect(),
            &done_tx,
            TrieNodeEpoch::new(1),
            false,
        );
        drop(done_tx);

        let mut batch_lens = Vec::new();
        let mut returned = Vec::new();
        for message in done_rx {
            let StorageJobMessage::Done(batch) = message else { panic!("no pass panicked") };
            batch_lens.push(batch.len());
            returned.extend(batch.into_iter().map(|done| done.address));
        }

        assert_eq!(returned, addresses, "every payload comes back, in the order it was queued");
        assert_eq!(batch_lens[..4], [1, 2, 4, 8], "the first payload does not wait for the rest");
        assert!(
            batch_lens.iter().all(|len| *len <= MAX_STORAGE_RETURN_BATCH),
            "batches stay bounded: {batch_lens:?}"
        );
    }

    #[test]
    fn a_group_queued_for_a_cancelled_task_is_dropped() {
        let (done_tx, done_rx) = crossbeam_channel::unbounded();
        drop(done_rx);

        // A cancelled task drops its receiver, so the shard has nowhere to hand payloads back
        // to. It must return instead of running the whole group for nobody.
        run_storage_jobs(
            (0..8).map(|index| storage_job(B256::repeat_byte(index))).collect(),
            &done_tx,
            TrieNodeEpoch::new(1),
            false,
        );
    }

    #[test]
    fn a_shard_thread_survives_a_panicking_job() {
        let pool = storage_shard_pool();
        let shard = pool.shard_of(&B256::repeat_byte(0xc3));

        pool.submit(shard, || panic!("pass panicked"));

        let (alive_tx, alive_rx) = crossbeam_channel::bounded(1);
        pool.submit(shard, move || {
            let _ = alive_tx.send(());
        });
        alive_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the shard must keep serving its queue");
    }

    #[test]
    fn a_key_stays_counted_once_while_it_moves_through_the_round_buckets() {
        let mut rounds = ProofRounds::default();
        rounds.first_target(ProofV2TargetParent::new(3));
        rounds.first_target(ProofV2TargetParent::NONE);
        assert_eq!(rounds.keys(), 2);
        assert_eq!(rounds.multi_round_keys(), 0);
        assert_eq!(rounds.root_targets, 1);
        assert_eq!(rounds.first_targets_by_parent_depth[3], 1);
        assert_eq!(rounds.parent_depth_sum, 3);

        // One key needs more targets than there are buckets; the last bucket absorbs them.
        for round in 1..=(PROOF_ROUND_BUCKETS as u8 + 1) {
            rounds.repeat_target(round);
            assert_eq!(rounds.keys(), 2, "round {round} lost or duplicated a key");
        }
        assert_eq!(rounds.multi_round_keys(), 1);
        assert_eq!(rounds.keys_by_round[PROOF_ROUND_BUCKETS - 1], 1);

        rounds.dropped_target(ProofV2TargetParent::new(5), ProofV2TargetParent::new(3));
        rounds.dropped_target(ProofV2TargetParent::new(3), ProofV2TargetParent::new(3));
        assert_eq!(rounds.dropped_deeper_parent, 1);
        assert_eq!(rounds.dropped_same_parent, 1);

        let mut total = ProofRounds::default();
        total.merge(&rounds);
        total.merge(&rounds);
        assert_eq!(total.keys(), 4);
        assert_eq!(total.multi_round_keys(), 2);
        assert_eq!(total.parent_depth_sum, 6);
    }
}
