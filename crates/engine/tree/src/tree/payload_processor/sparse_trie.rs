//! Sparse Trie task related functionality.

use std::{sync::Arc, time::Duration};

use crate::tree::{
    multiproof::{
        dispatch_with_chunking, evm_state_to_hashed_post_state, StateRootComputeOutcome,
        StateRootMessage, DEFAULT_MAX_TARGETS_FOR_CHUNKING,
    },
    payload_processor::multiproof::MultiProofTaskMetrics,
};
use alloy_primitives::B256;
use alloy_rlp::{Decodable, Encodable};
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use reth_primitives_traits::{Account, FastInstant as Instant};
use reth_tasks::Runtime;
use reth_trie::{
    updates::TrieUpdates, DecodedMultiProofV2, HashedPostState, TrieAccount, EMPTY_ROOT_HASH,
    TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_common::{MultiProofTargetsV2, ProofV2Target};
use reth_trie_parallel::{
    proof_task::{
        AccountMultiproofInput, ProofResultContext, ProofResultMessage, ProofResultStats,
        ProofWorkerHandle,
    },
    root::ParallelStateRootError,
};
use reth_trie_sparse::{
    errors::{SparseStateTrieErrorKind, SparseTrieErrorKind, SparseTrieResult},
    ConfigurableSparseTrie, DeferredDrops, LeafUpdate, RevealableSparseTrie, SparseStateTrie,
    SparseTrie,
};
use revm_primitives::{hash_map::Entry, B256Map};
use tracing::{debug, debug_span, error, instrument, trace_span};

/// Maximum number of pending/prewarm updates that we accumulate in memory before actually applying.
const MAX_PENDING_UPDATES: usize = 100;

/// Account proofs can block on database/page-cache latency, so large account-only batches benefit
/// from keeping as many proof requests in flight as the configured worker cap allows.
const ACCOUNT_PROOF_TARGET_CHUNKS_PER_BURST_WORKER: usize = 1;
/// Storage proof jobs are spawned by account proof workers, so keep a smaller storage burst.
const STORAGE_PROOF_TARGET_CHUNKS_PER_BURST_WORKER: usize = 4;

/// Sparse trie task implementation that uses in-memory sparse trie data to schedule proof fetching.
pub(super) struct SparseTrieCacheTask<A = ConfigurableSparseTrie, S = ConfigurableSparseTrie> {
    /// Sender for proof results.
    proof_result_tx: CrossbeamSender<ProofResultMessage>,
    /// Receiver for proof results directly from workers.
    proof_result_rx: CrossbeamReceiver<ProofResultMessage>,
    /// Receives updates from execution and prewarming.
    updates: CrossbeamReceiver<SparseTrieTaskMessage>,
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
    /// account -> lowest `min_len` requested.
    fetched_account_targets: B256Map<u8>,
    /// Cache of storage proof targets that have already been fetched/requested from the proof
    /// workers. account -> slot -> lowest `min_len` requested.
    fetched_storage_targets: B256Map<B256Map<u8>>,
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
        updates: CrossbeamReceiver<StateRootMessage>,
        proof_worker_handle: ProofWorkerHandle,
        metrics: MultiProofTaskMetrics,
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
            proof_worker_handle,
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
            pending_updates: Default::default(),
            metrics,
        }
    }

    /// Runs the hashing task that drains updates from the channel and converts them to
    /// `HashedPostState` in parallel.
    fn run_hashing_task(
        updates: CrossbeamReceiver<StateRootMessage>,
        hashed_state_tx: CrossbeamSender<SparseTrieTaskMessage>,
        metrics: MultiProofTaskMetrics,
    ) {
        let mut total_idle_time = std::time::Duration::ZERO;
        let mut idle_start = Instant::now();

        while let Ok(message) = updates.recv() {
            total_idle_time += idle_start.elapsed();

            let msg = match message {
                StateRootMessage::PrefetchProofs(targets) => {
                    let account_targets = targets.account_targets.len();
                    let storage_targets =
                        targets.storage_targets.values().map(Vec::len).sum::<usize>();
                    let storage_jobs = targets.storage_targets.len();
                    debug!(
                        target: "engine::tree::payload_processor::sparse_trie",
                        account_targets,
                        storage_targets,
                        storage_jobs,
                        "Hashing task forwarded prefetch proof targets"
                    );
                    SparseTrieTaskMessage::PrefetchProofs(targets)
                }
                StateRootMessage::StateUpdate(_, state) => {
                    let _span = trace_span!(target: "engine::tree::payload_processor::sparse_trie", "hashing_state_update", n = state.len()).entered();
                    let state_len = state.len();
                    let hash_start = Instant::now();
                    let hashed = evm_state_to_hashed_post_state(state);
                    let hash_elapsed = hash_start.elapsed();
                    let hashed_stats = HashedStateStats::new(&hashed);
                    debug!(
                        target: "engine::tree::payload_processor::sparse_trie",
                        state_len,
                        accounts = hashed_stats.accounts,
                        storage_addresses = hashed_stats.storage_addresses,
                        storage_slots = hashed_stats.storage_slots,
                        hash_elapsed_us = hash_elapsed.as_micros(),
                        "Hashing task converted state update"
                    );
                    SparseTrieTaskMessage::HashedState(hashed)
                }
                StateRootMessage::FinishedStateUpdates => {
                    debug!(
                        target: "engine::tree::payload_processor::sparse_trie",
                        "Hashing task forwarded finished state updates"
                    );
                    SparseTrieTaskMessage::FinishedStateUpdates
                }
                StateRootMessage::BlockAccessList(_) => {
                    debug!(
                        target: "engine::tree::payload_processor::sparse_trie",
                        "Hashing task ignored block access list message"
                    );
                    idle_start = Instant::now();
                    continue;
                }
                StateRootMessage::HashedStateUpdate(state) => {
                    let hashed_stats = HashedStateStats::new(&state);
                    debug!(
                        target: "engine::tree::payload_processor::sparse_trie",
                        accounts = hashed_stats.accounts,
                        storage_addresses = hashed_stats.storage_addresses,
                        storage_slots = hashed_stats.storage_slots,
                        "Hashing task forwarded pre-hashed state update"
                    );
                    SparseTrieTaskMessage::HashedState(state)
                }
            };
            let send_start = Instant::now();
            if hashed_state_tx.send(msg).is_err() {
                break;
            }
            debug!(
                target: "engine::tree::payload_processor::sparse_trie",
                send_elapsed_us = send_start.elapsed().as_micros(),
                "Hashing task sent sparse trie message"
            );

            idle_start = Instant::now();
        }

        metrics.hashing_task_idle_time_seconds.record(total_idle_time.as_secs_f64());
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
        max_hot_slots: usize,
        max_hot_accounts: usize,
        max_nodes_capacity: usize,
        max_values_capacity: usize,
        disable_pruning: bool,
        updates: &TrieUpdates,
    ) -> (SparseStateTrie<A, S>, DeferredDrops) {
        let Self { mut trie, .. } = self;
        trie.commit_updates(updates);
        if !disable_pruning {
            trie.prune(max_hot_slots, max_hot_accounts);
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

        let mut total_idle_time = std::time::Duration::ZERO;
        let mut idle_start = Instant::now();

        loop {
            let mut t = Instant::now();
            crossbeam_channel::select_biased! {
                recv(self.updates) -> message => {
                    let wake = Instant::now();

                    let update = match message {
                        Ok(m) => m,
                        Err(_) => {
                            return Err(ParallelStateRootError::Other(
                                "updates channel disconnected before state root calculation".to_string(),
                            ))
                        }
                    };

                    total_idle_time += wake.duration_since(idle_start);
                    self.metrics
                        .sparse_trie_channel_wait_duration_histogram
                        .record(wake.duration_since(t));

                    let message_stats = SparseTrieTaskMessageStats::new(&update);
                    let on_message_start = Instant::now();
                    self.on_message(update);
                    self.pending_updates += 1;
                    debug!(
                        target: "engine::tree::payload_processor::sparse_trie",
                        message_kind = message_stats.kind,
                        accounts = message_stats.accounts,
                        storage_addresses = message_stats.storage_addresses,
                        storage_slots = message_stats.storage_slots,
                        prefetch_account_targets = message_stats.prefetch_account_targets,
                        prefetch_storage_targets = message_stats.prefetch_storage_targets,
                        prefetch_storage_jobs = message_stats.prefetch_storage_jobs,
                        channel_wait_us = wake.duration_since(idle_start).as_micros(),
                        on_message_elapsed_us = on_message_start.elapsed().as_micros(),
                        pending_updates = self.pending_updates,
                        finished_state_updates = self.finished_state_updates,
                        "Sparse trie task processed input message"
                    );
                }
                recv(self.proof_result_rx) -> message => {
                    let phase_end = Instant::now();
                    total_idle_time += phase_end.duration_since(idle_start);
                    self.metrics
                        .sparse_trie_channel_wait_duration_histogram
                        .record(phase_end.duration_since(t));
                    t = phase_end;

                    let Ok(message) = message else {
                        unreachable!("we own the sender half")
                    };

                    let mut proof_batch_stats = ProofResultBatchStats::default();
                    proof_batch_stats.record(&message);
                    let mut result = message.result?;
                    while let Ok(next) = self.proof_result_rx.try_recv() {
                        proof_batch_stats.record(&next);
                        let res = next.result?;
                        result.extend(res);
                    }

                    let phase_end = Instant::now();
                    let coalesce_elapsed = phase_end.duration_since(t);
                    self.metrics
                        .sparse_trie_proof_coalesce_duration_histogram
                        .record(coalesce_elapsed);
                    t = phase_end;

                    let reveal_start = Instant::now();
                    self.on_proof_result(result)?;
                    let reveal_elapsed = reveal_start.elapsed();
                    debug!(
                        target: "engine::tree::payload_processor::sparse_trie",
                        proof_results = proof_batch_stats.proof_results,
                        account_targets = proof_batch_stats.account_targets,
                        storage_targets = proof_batch_stats.storage_targets,
                        storage_jobs = proof_batch_stats.storage_jobs,
                        storage_wait_time_us = proof_batch_stats.storage_wait_time.as_micros(),
                        worker_proof_time_us = proof_batch_stats.proof_elapsed.as_micros(),
                        max_worker_elapsed_us = proof_batch_stats.max_elapsed.as_micros(),
                        coalesce_elapsed_us = coalesce_elapsed.as_micros(),
                        reveal_elapsed_us = reveal_elapsed.as_micros(),
                        "Processed proof result batch"
                    );
                    self.metrics
                        .sparse_trie_reveal_multiproof_duration_histogram
                        .record(reveal_elapsed);
                },
            }

            if self.updates.is_empty() && self.proof_result_rx.is_empty() {
                // If we don't have any pending messages, we can spend some time on computing
                // storage roots and promoting account updates.
                let mut dispatched_proof_jobs = self.dispatch_pending_targets();
                t = Instant::now();
                let process_start = Instant::now();
                self.process_new_updates()?;
                let process_elapsed = process_start.elapsed();
                let promote_start = Instant::now();
                self.promote_pending_account_updates()?;
                let promote_elapsed = promote_start.elapsed();
                let process_phase_elapsed = t.elapsed();
                self.metrics
                    .sparse_trie_process_updates_duration_histogram
                    .record(process_phase_elapsed);
                self.log_sparse_trie_phase(
                    "Processed idle sparse trie update phase",
                    process_elapsed,
                    Some(promote_elapsed),
                    process_phase_elapsed,
                );

                if self.finished_state_updates &&
                    self.account_updates.is_empty() &&
                    self.storage_updates.iter().all(|(_, updates)| updates.is_empty())
                {
                    break;
                }

                dispatched_proof_jobs += self.dispatch_pending_targets();

                // If there's still no pending work, spend some time pre-computing the account
                // trie upper hashes. Avoid doing speculative work immediately after proof dispatch:
                // proof results or new updates can arrive while `calculate_subtries` is running,
                // and delaying those messages is worse than overlapping this pre-computation.
                if dispatched_proof_jobs == 0 && self.proof_result_rx.is_empty() {
                    let calculate_start = Instant::now();
                    self.trie.calculate_subtries();
                    debug!(
                        target: "engine::tree::payload_processor::sparse_trie",
                        calculate_subtries_elapsed_us = calculate_start.elapsed().as_micros(),
                        "Calculated account subtries"
                    );
                }
            } else if self.updates.is_empty() || self.pending_updates > MAX_PENDING_UPDATES {
                // If we don't have any pending updates OR we've accumulated a lot already, apply
                // them to the trie,
                t = Instant::now();
                let process_start = Instant::now();
                self.process_new_updates()?;
                let process_elapsed = process_start.elapsed();
                let process_phase_elapsed = t.elapsed();
                self.metrics
                    .sparse_trie_process_updates_duration_histogram
                    .record(process_phase_elapsed);
                self.log_sparse_trie_phase(
                    "Processed sparse trie update phase",
                    process_elapsed,
                    None,
                    process_phase_elapsed,
                );
                self.dispatch_pending_targets();
            } else if self.pending_targets.len() > self.chunk_size {
                // Make sure to dispatch targets if we've accumulated a lot of them.
                self.dispatch_pending_targets();
            }

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
                return Err(ParallelStateRootError::Other(format!(
                    "could not calculate state root: {err:?}"
                )))
            }
        };

        #[cfg(feature = "trie-debug")]
        let debug_recorders = self.trie.take_debug_recorders();

        let end = Instant::now();
        let final_update_elapsed = end.duration_since(start);
        let total_elapsed = end.duration_since(now);
        self.metrics.sparse_trie_final_update_duration_histogram.record(final_update_elapsed);
        self.metrics.sparse_trie_total_duration_histogram.record(total_elapsed);
        debug!(
            target: "engine::tree::payload_processor::sparse_trie",
            final_update_elapsed_us = final_update_elapsed.as_micros(),
            total_elapsed_us = total_elapsed.as_micros(),
            sparse_trie_idle_time_us = total_idle_time.as_micros(),
            account_cache_hits = self.account_cache_hits,
            account_cache_misses = self.account_cache_misses,
            storage_cache_hits = self.storage_cache_hits,
            storage_cache_misses = self.storage_cache_misses,
            "Computed sparse trie state root"
        );

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
            #[cfg(feature = "trie-debug")]
            debug_recorders,
        })
    }

    fn log_sparse_trie_phase(
        &self,
        phase: &'static str,
        process_elapsed: Duration,
        promote_elapsed: Option<Duration>,
        total_elapsed: Duration,
    ) {
        debug!(
            target: "engine::tree::payload_processor::sparse_trie",
            phase,
            pending = ?SparseTriePendingStats::new(self),
            process_new_updates_elapsed_us = process_elapsed.as_micros(),
            promote_pending_account_updates_elapsed_us =
                promote_elapsed.map_or(0, |elapsed| elapsed.as_micros()),
            total_elapsed_us = total_elapsed.as_micros(),
            "Sparse trie phase completed"
        );
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
        for (address, storage) in hashed_state_update.storages {
            if !storage.storage.is_empty() {
                // Look up outer maps once per address instead of once per slot.
                let new_updates = self.new_storage_updates.entry(address).or_default();
                let mut existing_updates = self.storage_updates.get_mut(&address);

                for (slot, value) in storage.storage {
                    self.trie.record_slot_touch(address, slot);

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

        for (address, account) in hashed_state_update.accounts {
            self.trie.record_account_touch(address);

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
        let pending_updates = self.pending_updates;
        let stats_before = SparseTriePendingStats::new(self);
        self.pending_updates = 0;

        // Firstly apply all new storage and account updates to the tries.
        let leaf_start = Instant::now();
        self.process_leaf_updates(true)?;
        let leaf_elapsed = leaf_start.elapsed();

        let storage_merge_start = Instant::now();
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
        let storage_merge_elapsed = storage_merge_start.elapsed();

        let account_merge_start = Instant::now();
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
        let account_merge_elapsed = account_merge_start.elapsed();
        let stats_after = SparseTriePendingStats::new(self);
        debug!(
            target: "engine::tree::payload_processor::sparse_trie",
            pending_updates,
            before = ?stats_before,
            after = ?stats_after,
            leaf_update_elapsed_us = leaf_elapsed.as_micros(),
            storage_merge_elapsed_us = storage_merge_elapsed.as_micros(),
            account_merge_elapsed_us = account_merge_elapsed.as_micros(),
            "Processed new sparse trie updates"
        );

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
            trie.update_leaves(updates, |path, min_len| match fetched.entry(path) {
                Entry::Occupied(mut entry) => {
                    if min_len < *entry.get() {
                        entry.insert(min_len);
                        targets.push(ProofV2Target::new(path).with_min_len(min_len));
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(min_len);
                    targets.push(ProofV2Target::new(path).with_min_len(min_len));
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

        self.trie.trie_mut().update_leaves(account_updates, |target, min_len| {
            match self.fetched_account_targets.entry(target) {
                Entry::Occupied(mut entry) => {
                    if min_len < *entry.get() {
                        entry.insert(min_len);
                        self.pending_targets
                            .push_account_target(ProofV2Target::new(target).with_min_len(min_len));
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(min_len);
                    self.pending_targets
                        .push_account_target(ProofV2Target::new(target).with_min_len(min_len));
                }
            }
        })?;

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
        let start = Instant::now();
        let addresses_to_compute_roots: Vec<_> = self
            .storage_updates
            .iter()
            .filter_map(|(address, updates)| updates.is_empty().then_some(*address))
            .collect();
        let drained_storage_tries = addresses_to_compute_roots.len();

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
            debug!(
                target: "engine::tree::payload_processor::sparse_trie",
                drained_storage_tries,
                storage_roots_computed = 0usize,
                elapsed_us = start.elapsed().as_micros(),
                "Computed drained storage roots"
            );
            return;
        }

        let storage_roots_computed = tries_to_compute_roots.len();
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
        debug!(
            target: "engine::tree::payload_processor::sparse_trie",
            drained_storage_tries,
            storage_roots_computed,
            elapsed_us = start.elapsed().as_micros(),
            "Computed drained storage roots"
        );
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
        let start = Instant::now();
        let stats_before = SparseTriePendingStats::new(self);
        let leaf_start = Instant::now();
        self.process_leaf_updates(false)?;
        let leaf_elapsed = leaf_start.elapsed();

        if self.pending_account_updates.is_empty() {
            debug!(
                target: "engine::tree::payload_processor::sparse_trie",
                before = ?stats_before,
                after = ?SparseTriePendingStats::new(self),
                leaf_update_elapsed_us = leaf_elapsed.as_micros(),
                storage_root_elapsed_us = 0u128,
                account_leaf_update_elapsed_us = 0u128,
                promotion_iterations = 0usize,
                promoted_accounts = 0usize,
                total_elapsed_us = start.elapsed().as_micros(),
                "Promoted pending account updates"
            );
            return Ok(());
        }

        let storage_root_start = Instant::now();
        self.compute_drained_storage_roots();
        let storage_root_elapsed = storage_root_start.elapsed();

        let mut promotion_iterations = 0usize;
        let mut promoted_accounts = 0usize;
        let mut account_leaf_update_elapsed = Duration::ZERO;
        loop {
            promotion_iterations += 1;
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
            promoted_accounts += num_promoted;

            // Only exit when no new updates are processed.
            //
            // We need to keep iterating if any updates are being drained because that might
            // indicate that more pending account updates can be promoted.
            if num_promoted == 0 {
                break
            }

            let account_leaf_start = Instant::now();
            if !self.process_account_leaf_updates(false)? {
                account_leaf_update_elapsed += account_leaf_start.elapsed();
                break
            }
            account_leaf_update_elapsed += account_leaf_start.elapsed();
        }
        debug!(
            target: "engine::tree::payload_processor::sparse_trie",
            before = ?stats_before,
            after = ?SparseTriePendingStats::new(self),
            leaf_update_elapsed_us = leaf_elapsed.as_micros(),
            storage_root_elapsed_us = storage_root_elapsed.as_micros(),
            account_leaf_update_elapsed_us = account_leaf_update_elapsed.as_micros(),
            promotion_iterations,
            promoted_accounts,
            total_elapsed_us = start.elapsed().as_micros(),
            "Promoted pending account updates"
        );

        Ok(())
    }

    fn dispatch_pending_targets(&mut self) -> usize {
        if self.pending_targets.is_empty() {
            return 0;
        }

        let _span = trace_span!("dispatch_pending_targets").entered();
        let (targets, target_counts) = self.pending_targets.take();
        let chunking_length = target_counts.total();
        let storage_jobs = targets.storage_targets.len();
        let current_storage_worker_count = self.proof_worker_handle.total_storage_workers();
        let current_account_worker_count = self.proof_worker_handle.total_account_workers();
        let (desired_storage_worker_count, desired_account_worker_count) = self
            .desired_worker_capacity(
                target_counts,
                current_storage_worker_count,
                current_account_worker_count,
            );
        let ensure_capacity_start = Instant::now();
        self.proof_worker_handle
            .ensure_worker_capacity(desired_storage_worker_count, desired_account_worker_count);
        let ensure_capacity_elapsed = ensure_capacity_start.elapsed();
        let active_storage_worker_count = self.proof_worker_handle.total_storage_workers();
        let active_account_worker_count = self.proof_worker_handle.total_account_workers();
        let has_multiple_idle_account_workers =
            self.proof_worker_handle.has_multiple_idle_account_workers();
        let has_multiple_idle_storage_workers =
            self.proof_worker_handle.has_multiple_idle_storage_workers();

        let dispatch_start = Instant::now();
        let chunks_dispatched = dispatch_with_chunking(
            targets,
            chunking_length,
            self.chunk_size,
            self.max_targets_for_chunking,
            has_multiple_idle_account_workers,
            has_multiple_idle_storage_workers,
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
        debug!(
            target: "engine::tree::payload_processor::sparse_trie",
            account_targets = target_counts.account,
            storage_targets = target_counts.storage,
            storage_jobs,
            total_targets = chunking_length,
            chunk_size = self.chunk_size,
            max_targets_for_chunking = self.max_targets_for_chunking,
            has_multiple_idle_account_workers,
            has_multiple_idle_storage_workers,
            current_storage_workers = current_storage_worker_count,
            current_account_workers = current_account_worker_count,
            desired_storage_workers = desired_storage_worker_count,
            desired_account_workers = desired_account_worker_count,
            active_storage_workers = active_storage_worker_count,
            active_account_workers = active_account_worker_count,
            spawned_storage_workers =
                active_storage_worker_count.saturating_sub(current_storage_worker_count),
            spawned_account_workers =
                active_account_worker_count.saturating_sub(current_account_worker_count),
            chunks_dispatched,
            ensure_capacity_elapsed_us = ensure_capacity_elapsed.as_micros(),
            dispatch_elapsed_us = dispatch_start.elapsed().as_micros(),
            "Dispatched pending proof targets"
        );
        chunks_dispatched
    }

    fn desired_worker_capacity(
        &self,
        target_counts: PendingTargetCounts,
        current_storage_workers: usize,
        current_account_workers: usize,
    ) -> (usize, usize) {
        if target_counts.total() <= self.max_targets_for_chunking {
            return (current_storage_workers, current_account_workers);
        }

        let chunk_size = self.chunk_size.max(1);
        let target_chunks = target_counts.total().div_ceil(chunk_size);
        let storage_chunks = target_counts.storage.div_ceil(chunk_size);

        let desired_account_workers = current_account_workers
            .max(target_chunks.div_ceil(ACCOUNT_PROOF_TARGET_CHUNKS_PER_BURST_WORKER));
        let desired_storage_workers = current_storage_workers
            .max(storage_chunks.div_ceil(STORAGE_PROOF_TARGET_CHUNKS_PER_BURST_WORKER));

        (desired_storage_workers, desired_account_workers)
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
    /// Number of account proof targets currently queued.
    account_len: usize,
    /// Number of storage proof targets currently queued.
    storage_len: usize,
}

#[derive(Clone, Copy, Debug)]
struct PendingTargetCounts {
    /// Number of account proof targets currently queued.
    account: usize,
    /// Number of storage proof targets currently queued.
    storage: usize,
}

impl PendingTargetCounts {
    const fn total(self) -> usize {
        self.account + self.storage
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct HashedStateStats {
    accounts: usize,
    storage_addresses: usize,
    storage_slots: usize,
}

impl HashedStateStats {
    fn new(state: &HashedPostState) -> Self {
        Self {
            accounts: state.accounts.len(),
            storage_addresses: state.storages.len(),
            storage_slots: state.storages.values().map(|storage| storage.storage.len()).sum(),
        }
    }
}

struct SparseTrieTaskMessageStats {
    kind: &'static str,
    accounts: usize,
    storage_addresses: usize,
    storage_slots: usize,
    prefetch_account_targets: usize,
    prefetch_storage_targets: usize,
    prefetch_storage_jobs: usize,
}

impl SparseTrieTaskMessageStats {
    fn new(message: &SparseTrieTaskMessage) -> Self {
        match message {
            SparseTrieTaskMessage::PrefetchProofs(targets) => Self {
                kind: "prefetch_proofs",
                accounts: 0,
                storage_addresses: 0,
                storage_slots: 0,
                prefetch_account_targets: targets.account_targets.len(),
                prefetch_storage_targets: targets.storage_targets.values().map(Vec::len).sum(),
                prefetch_storage_jobs: targets.storage_targets.len(),
            },
            SparseTrieTaskMessage::HashedState(state) => {
                let HashedStateStats { accounts, storage_addresses, storage_slots } =
                    HashedStateStats::new(state);
                Self {
                    kind: "hashed_state",
                    accounts,
                    storage_addresses,
                    storage_slots,
                    prefetch_account_targets: 0,
                    prefetch_storage_targets: 0,
                    prefetch_storage_jobs: 0,
                }
            }
            SparseTrieTaskMessage::FinishedStateUpdates => Self {
                kind: "finished_state_updates",
                accounts: 0,
                storage_addresses: 0,
                storage_slots: 0,
                prefetch_account_targets: 0,
                prefetch_storage_targets: 0,
                prefetch_storage_jobs: 0,
            },
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct SparseTriePendingStats {
    pending_updates: usize,
    new_account_updates: usize,
    account_updates: usize,
    pending_account_updates: usize,
    new_storage_tries: usize,
    new_storage_updates: usize,
    storage_tries: usize,
    storage_updates: usize,
    pending_targets: usize,
}

impl SparseTriePendingStats {
    fn new<A, S>(task: &SparseTrieCacheTask<A, S>) -> Self {
        Self {
            pending_updates: task.pending_updates,
            new_account_updates: task.new_account_updates.len(),
            account_updates: task.account_updates.len(),
            pending_account_updates: task.pending_account_updates.len(),
            new_storage_tries: task.new_storage_updates.len(),
            new_storage_updates: task
                .new_storage_updates
                .values()
                .map(|updates| updates.len())
                .sum(),
            storage_tries: task.storage_updates.len(),
            storage_updates: task.storage_updates.values().map(|updates| updates.len()).sum(),
            pending_targets: task.pending_targets.len(),
        }
    }
}

#[derive(Default)]
struct ProofResultBatchStats {
    proof_results: usize,
    account_targets: usize,
    storage_targets: usize,
    storage_jobs: usize,
    proof_elapsed: Duration,
    storage_wait_time: Duration,
    max_elapsed: Duration,
}

impl ProofResultBatchStats {
    fn record(&mut self, message: &ProofResultMessage) {
        let ProofResultStats {
            account_targets,
            storage_targets,
            storage_jobs,
            proof_elapsed,
            storage_wait_time,
        } = message.stats;

        self.proof_results += 1;
        self.account_targets += account_targets;
        self.storage_targets += storage_targets;
        self.storage_jobs += storage_jobs;
        self.proof_elapsed += proof_elapsed;
        self.storage_wait_time += storage_wait_time;
        self.max_elapsed = self.max_elapsed.max(message.elapsed);
    }
}

impl PendingTargets {
    /// Returns the number of pending targets.
    const fn len(&self) -> usize {
        self.account_len + self.storage_len
    }

    /// Returns `true` if there are no pending targets.
    const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Takes the pending targets, replacing with empty defaults.
    fn take(&mut self) -> (MultiProofTargetsV2, PendingTargetCounts) {
        let counts = PendingTargetCounts { account: self.account_len, storage: self.storage_len };
        self.account_len = 0;
        self.storage_len = 0;
        (std::mem::take(&mut self.targets), counts)
    }

    /// Adds a target to the account targets.
    fn push_account_target(&mut self, target: ProofV2Target) {
        self.targets.account_targets.push(target);
        self.account_len += 1;
    }

    /// Extends storage targets for the given address.
    fn extend_storage_targets(&mut self, address: &B256, targets: Vec<ProofV2Target>) {
        self.storage_len += targets.len();
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
                MultiProofTaskMetrics::default(),
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

        let default_trie = RevealableSparseTrie::blind_from(ConfigurableSparseTrie::Arena(
            ArenaParallelSparseTrie::default(),
        ));
        let trie = SparseStateTrie::default()
            .with_accounts_trie(default_trie.clone())
            .with_default_storage_trie(default_trie)
            .with_updates(true);

        let parent_state_root = B256::from([0x55; 32]);
        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let mut task = SparseTrieCacheTask::new_with_trie(
            &runtime,
            updates_rx,
            proof_worker_handle,
            MultiProofTaskMetrics::default(),
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
}
