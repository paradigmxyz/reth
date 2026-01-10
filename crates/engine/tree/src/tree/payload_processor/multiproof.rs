//! Multiproof task related functionality.

use crate::tree::payload_processor::bal::bal_to_hashed_post_state;
use alloy_eip7928::BlockAccessList;
use alloy_evm::block::StateChangeSource;
use alloy_primitives::{keccak256, map::HashSet, B256};
use crossbeam_channel::{unbounded, Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use derive_more::derive::Deref;
use metrics::{Gauge, Histogram};
use reth_metrics::Metrics;
use reth_provider::AccountReader;
use reth_revm::state::EvmState;
use reth_trie::{
    added_removed_keys::MultiAddedRemovedKeys, DecodedMultiProof, HashedPostState, HashedStorage,
    MultiProofTargets,
};
use reth_trie_parallel::{
    proof::ParallelProof,
    proof_task::{
        AccountMultiproofInput, ProofResultContext, ProofResultMessage, ProofWorkerHandle,
    },
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use tracing::{debug, error, instrument, trace};

/// Source of state changes, either from EVM execution or from a Block Access List.
#[derive(Clone, Copy)]
pub enum Source {
    /// State changes from EVM execution.
    Evm(StateChangeSource),
    /// State changes from Block Access List (EIP-7928).
    BlockAccessList,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Evm(source) => source.fmt(f),
            Self::BlockAccessList => f.write_str("BlockAccessList"),
        }
    }
}

impl From<StateChangeSource> for Source {
    fn from(source: StateChangeSource) -> Self {
        Self::Evm(source)
    }
}

/// Maximum number of targets to batch together for prefetch batching.
/// Prefetches are just proof requests (no state merging), so we allow a higher cap than state
/// updates
const PREFETCH_MAX_BATCH_TARGETS: usize = 512;

/// Maximum number of prefetch messages to batch together.
/// Prevents excessive batching even with small messages.
const PREFETCH_MAX_BATCH_MESSAGES: usize = 16;

/// The default max targets, for limiting the number of account and storage proof targets to be
/// fetched by a single worker. If exceeded, chunking is forced regardless of worker availability.
const DEFAULT_MAX_TARGETS_FOR_CHUNKING: usize = 300;

/// A trie update that can be applied to sparse trie alongside the proofs for touched parts of the
/// state.
#[derive(Default, Debug)]
pub struct SparseTrieUpdate {
    /// The state update that was used to calculate the proof
    pub(crate) state: HashedPostState,
    /// The calculated multiproof
    pub(crate) multiproof: DecodedMultiProof,
}

impl SparseTrieUpdate {
    /// Returns true if the update is empty.
    pub(super) fn is_empty(&self) -> bool {
        self.state.is_empty() && self.multiproof.is_empty()
    }

    /// Construct update from multiproof.
    #[cfg(test)]
    pub(super) fn from_multiproof(multiproof: reth_trie::MultiProof) -> alloy_rlp::Result<Self> {
        Ok(Self { multiproof: multiproof.try_into()?, ..Default::default() })
    }

    /// Extend update with contents of the other.
    pub(super) fn extend(&mut self, other: Self) {
        self.state.extend(other.state);
        self.multiproof.extend(other.multiproof);
    }
}

/// Messages used internally by the multi proof task.
#[derive(Debug)]
pub(super) enum MultiProofMessage {
    /// Prefetch proof targets
    PrefetchProofs(MultiProofTargets),
    /// New state update from transaction execution with its source
    StateUpdate(Source, EvmState),
    /// State update that can be applied to the sparse trie without any new proofs.
    ///
    /// It can be the case when all accounts and storage slots from the state update were already
    /// fetched and revealed.
    EmptyProof {
        /// The index of this proof in the sequence of state updates
        sequence_number: u64,
        /// The state update that was used to calculate the proof
        state: HashedPostState,
    },
    /// Block Access List (EIP-7928; BAL) containing complete state changes for the block.
    ///
    /// When received, the task generates a single state update from the BAL and processes it.
    /// No further messages are expected after receiving this variant.
    BlockAccessList(Arc<BlockAccessList>),
    /// Signals state update stream end.
    ///
    /// This is triggered by block execution, indicating that no additional state updates are
    /// expected.
    FinishedStateUpdates,
}

/// Handle to track proof calculation ordering.
#[derive(Debug, Default)]
struct ProofSequencer {
    /// The next proof sequence number to be produced.
    next_sequence: u64,
    /// The next sequence number expected to be delivered.
    next_to_deliver: u64,
    /// Buffer for out-of-order proofs and corresponding state updates
    pending_proofs: BTreeMap<u64, SparseTrieUpdate>,
}

impl ProofSequencer {
    /// Gets the next sequence number and increments the counter
    const fn next_sequence(&mut self) -> u64 {
        let seq = self.next_sequence;
        self.next_sequence += 1;
        seq
    }

    /// Adds a proof with the corresponding state update and returns all sequential proofs and state
    /// updates if we have a continuous sequence
    fn add_proof(&mut self, sequence: u64, update: SparseTrieUpdate) -> Vec<SparseTrieUpdate> {
        if sequence >= self.next_to_deliver {
            self.pending_proofs.insert(sequence, update);
        }

        let mut consecutive_proofs = Vec::with_capacity(self.pending_proofs.len());
        let mut current_sequence = self.next_to_deliver;

        // keep collecting proofs and state updates as long as we have consecutive sequence numbers
        while let Some(pending) = self.pending_proofs.remove(&current_sequence) {
            consecutive_proofs.push(pending);
            current_sequence += 1;
        }

        self.next_to_deliver += consecutive_proofs.len() as u64;

        consecutive_proofs
    }

    /// Returns true if we still have pending proofs
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending_proofs.is_empty()
    }
}

/// A wrapper for the sender that signals completion when dropped.
///
/// This type is intended to be used in combination with the evm executor statehook.
/// This should trigger once the block has been executed (after) the last state update has been
/// sent. This triggers the exit condition of the multi proof task.
#[derive(Deref, Debug)]
pub(super) struct StateHookSender(CrossbeamSender<MultiProofMessage>);

impl StateHookSender {
    pub(crate) const fn new(inner: CrossbeamSender<MultiProofMessage>) -> Self {
        Self(inner)
    }
}

impl Drop for StateHookSender {
    fn drop(&mut self) {
        // Send completion signal when the sender is dropped
        let _ = self.0.send(MultiProofMessage::FinishedStateUpdates);
    }
}

pub(crate) fn evm_state_to_hashed_post_state(update: EvmState) -> HashedPostState {
    let mut hashed_state = HashedPostState::with_capacity(update.len());

    for (address, account) in update {
        if account.is_touched() {
            let hashed_address = keccak256(address);
            trace!(target: "engine::tree::payload_processor::multiproof", ?address, ?hashed_address, "Adding account to state update");

            let destroyed = account.is_selfdestructed();
            let info = if destroyed { None } else { Some(account.info.into()) };
            hashed_state.accounts.insert(hashed_address, info);

            let mut changed_storage_iter = account
                .storage
                .into_iter()
                .filter(|(_slot, value)| value.is_changed())
                .map(|(slot, value)| (keccak256(B256::from(slot)), value.present_value))
                .peekable();

            if destroyed {
                hashed_state.storages.insert(hashed_address, HashedStorage::new(true));
            } else if changed_storage_iter.peek().is_some() {
                hashed_state
                    .storages
                    .insert(hashed_address, HashedStorage::from_iter(false, changed_storage_iter));
            }
        }
    }

    hashed_state
}

/// Input parameters for dispatching a multiproof calculation.
#[derive(Debug)]
struct MultiproofInput {
    source: Option<Source>,
    hashed_state_update: HashedPostState,
    proof_targets: MultiProofTargets,
    proof_sequence_number: u64,
    state_root_message_sender: CrossbeamSender<MultiProofMessage>,
    multi_added_removed_keys: Option<Arc<MultiAddedRemovedKeys>>,
}

impl MultiproofInput {
    /// Destroys the input and sends a [`MultiProofMessage::EmptyProof`] message to the sender.
    fn send_empty_proof(self) {
        let _ = self.state_root_message_sender.send(MultiProofMessage::EmptyProof {
            sequence_number: self.proof_sequence_number,
            state: self.hashed_state_update,
        });
    }
}

/// Coordinates multiproof dispatch between `MultiProofTask` and the parallel trie workers.
///
/// # Flow
/// 1. `MultiProofTask` asks the manager to dispatch either storage or account proof work.
/// 2. The manager builds the request, clones `proof_result_tx`, and hands everything to
///    [`ProofWorkerHandle`].
/// 3. A worker finishes the proof and sends a [`ProofResultMessage`] through the channel included
///    in the job.
/// 4. `MultiProofTask` consumes the message from the same channel and sequences it with
///    `ProofSequencer`.
#[derive(Debug)]
pub struct MultiproofManager {
    /// Handle to the proof worker pools (storage and account).
    proof_worker_handle: ProofWorkerHandle,
    /// Channel sender cloned into each dispatched job so workers can send back the
    /// `ProofResultMessage`.
    proof_result_tx: CrossbeamSender<ProofResultMessage>,
    /// Metrics
    metrics: MultiProofTaskMetrics,
    /// Whether to use V2 storage proofs
    v2_proofs_enabled: bool,
}

impl MultiproofManager {
    /// Creates a new [`MultiproofManager`].
    fn new(
        metrics: MultiProofTaskMetrics,
        proof_worker_handle: ProofWorkerHandle,
        proof_result_tx: CrossbeamSender<ProofResultMessage>,
    ) -> Self {
        // Initialize the max worker gauges with the worker pool sizes
        metrics.max_storage_workers.set(proof_worker_handle.total_storage_workers() as f64);
        metrics.max_account_workers.set(proof_worker_handle.total_account_workers() as f64);

        let v2_proofs_enabled = proof_worker_handle.v2_proofs_enabled();

        Self { metrics, proof_worker_handle, proof_result_tx, v2_proofs_enabled }
    }

    /// Dispatches a new multiproof calculation to worker pools.
    fn dispatch(&self, input: MultiproofInput) {
        // If there are no proof targets, we can just send an empty multiproof back immediately
        if input.proof_targets.is_empty() {
            trace!(
                sequence_number = input.proof_sequence_number,
                "No proof targets, sending empty multiproof back immediately"
            );
            input.send_empty_proof();
            return;
        }

        self.dispatch_multiproof(input);
    }

    /// Signals that a multiproof calculation has finished.
    fn on_calculation_complete(&self) {
        self.metrics
            .active_storage_workers_histogram
            .record(self.proof_worker_handle.active_storage_workers() as f64);
        self.metrics
            .active_account_workers_histogram
            .record(self.proof_worker_handle.active_account_workers() as f64);
        self.metrics
            .pending_storage_multiproofs_histogram
            .record(self.proof_worker_handle.pending_storage_tasks() as f64);
        self.metrics
            .pending_account_multiproofs_histogram
            .record(self.proof_worker_handle.pending_account_tasks() as f64);
    }

    /// Dispatches a single multiproof calculation to worker pool.
    fn dispatch_multiproof(&self, multiproof_input: MultiproofInput) {
        let MultiproofInput {
            source,
            hashed_state_update,
            proof_targets,
            proof_sequence_number,
            state_root_message_sender: _,
            multi_added_removed_keys,
        } = multiproof_input;

        let account_targets = proof_targets.len();
        let storage_targets = proof_targets.values().map(|slots| slots.len()).sum::<usize>();

        trace!(
            target: "engine::tree::payload_processor::multiproof",
            proof_sequence_number,
            ?proof_targets,
            account_targets,
            storage_targets,
            ?source,
            "Dispatching multiproof to workers"
        );

        let start = Instant::now();

        // Extend prefix sets with targets
        let frozen_prefix_sets =
            ParallelProof::extend_prefix_sets_with_targets(&Default::default(), &proof_targets);

        // Dispatch account multiproof to worker pool with result sender
        let input = AccountMultiproofInput {
            targets: proof_targets,
            prefix_sets: frozen_prefix_sets,
            collect_branch_node_masks: true,
            multi_added_removed_keys,
            // Workers will send ProofResultMessage directly to proof_result_rx
            proof_result_sender: ProofResultContext::new(
                self.proof_result_tx.clone(),
                proof_sequence_number,
                hashed_state_update,
                start,
            ),
            v2_proofs_enabled: self.v2_proofs_enabled,
        };

        if let Err(e) = self.proof_worker_handle.dispatch_account_multiproof(input) {
            error!(target: "engine::tree::payload_processor::multiproof", ?e, "Failed to dispatch account multiproof");
            return;
        }

        self.metrics
            .active_storage_workers_histogram
            .record(self.proof_worker_handle.active_storage_workers() as f64);
        self.metrics
            .active_account_workers_histogram
            .record(self.proof_worker_handle.active_account_workers() as f64);
        self.metrics
            .pending_storage_multiproofs_histogram
            .record(self.proof_worker_handle.pending_storage_tasks() as f64);
        self.metrics
            .pending_account_multiproofs_histogram
            .record(self.proof_worker_handle.pending_account_tasks() as f64);
    }
}

#[derive(Metrics, Clone)]
#[metrics(scope = "tree.root")]
pub(crate) struct MultiProofTaskMetrics {
    /// Histogram of active storage workers processing proofs.
    pub active_storage_workers_histogram: Histogram,
    /// Histogram of active account workers processing proofs.
    pub active_account_workers_histogram: Histogram,
    /// Gauge for the maximum number of storage workers in the pool.
    pub max_storage_workers: Gauge,
    /// Gauge for the maximum number of account workers in the pool.
    pub max_account_workers: Gauge,
    /// Histogram of pending storage multiproofs in the queue.
    pub pending_storage_multiproofs_histogram: Histogram,
    /// Histogram of pending account multiproofs in the queue.
    pub pending_account_multiproofs_histogram: Histogram,

    /// Histogram of the number of prefetch proof target accounts.
    pub prefetch_proof_targets_accounts_histogram: Histogram,
    /// Histogram of the number of prefetch proof target storages.
    pub prefetch_proof_targets_storages_histogram: Histogram,
    /// Histogram of the number of prefetch proof target chunks.
    pub prefetch_proof_chunks_histogram: Histogram,

    /// Histogram of the number of state update proof target accounts.
    pub state_update_proof_targets_accounts_histogram: Histogram,
    /// Histogram of the number of state update proof target storages.
    pub state_update_proof_targets_storages_histogram: Histogram,
    /// Histogram of the number of state update proof target chunks.
    pub state_update_proof_chunks_histogram: Histogram,

    /// Histogram of prefetch proof batch sizes (number of messages merged).
    pub prefetch_batch_size_histogram: Histogram,

    /// Histogram of proof calculation durations.
    pub proof_calculation_duration_histogram: Histogram,

    /// Histogram of sparse trie update durations.
    pub sparse_trie_update_duration_histogram: Histogram,
    /// Histogram of sparse trie final update durations.
    pub sparse_trie_final_update_duration_histogram: Histogram,
    /// Histogram of sparse trie total durations.
    pub sparse_trie_total_duration_histogram: Histogram,

    /// Histogram of state updates received.
    pub state_updates_received_histogram: Histogram,
    /// Histogram of proofs processed.
    pub proofs_processed_histogram: Histogram,
    /// Histogram of total time spent in the multiproof task.
    pub multiproof_task_total_duration_histogram: Histogram,
    /// Total time spent waiting for the first state update or prefetch request.
    pub first_update_wait_time_histogram: Histogram,
    /// Total time spent waiting for the last proof result.
    pub last_proof_wait_time_histogram: Histogram,
}

/// Standalone task that receives a transaction state stream and updates relevant
/// data structures to calculate state root.
///
/// ## Architecture: Dual-Channel Multiproof System
///
/// This task orchestrates parallel proof computation using a dual-channel architecture that
/// separates control messages from proof computation results:
///
/// ```text
/// ┌─────────────────────────────────────────────────────────────────┐
/// │                        MultiProofTask                            │
/// │                  Event Loop (crossbeam::select!)                 │
/// └──┬──────────────────────────────────────────────────────────▲───┘
///    │                                                           │
///    │ (1) Send proof request                                   │
///    │     via tx (control channel)                             │
///    │                                                           │
///    ▼                                                           │
/// ┌──────────────────────────────────────────────────────────────┐ │
/// │             MultiproofManager                                │ │
/// │  - Deduplicates against fetched_proof_targets                │ │
/// │  - Routes to appropriate worker pool                         │ │
/// └──┬───────────────────────────────────────────────────────────┘ │
///    │                                                             │
///    │ (2) Dispatch to workers                                    │
///    │     OR send EmptyProof (fast path)                         │
///    ▼                                                             │
/// ┌──────────────────────────────────────────────────────────────┐ │
/// │              ProofWorkerHandle                                │ │
/// │  ┌─────────────────────┐   ┌────────────────────────┐        │ │
/// │  │ Storage Worker Pool │   │ Account Worker Pool     │        │ │
/// │  │ (spawn_blocking)    │   │ (spawn_blocking)        │        │ │
/// │  └─────────────────────┘   └────────────────────────┘        │ │
/// └──┬───────────────────────────────────────────────────────────┘ │
///    │                                                             │
///    │ (3) Compute proofs in parallel                             │
///    │     Send results back                                      │
///    │                                                             │
///    ▼                                                             │
/// ┌──────────────────────────────────────────────────────────────┐ │
/// │  proof_result_tx (crossbeam unbounded channel)                │ │
/// │    → ProofResultMessage { multiproof, sequence_number, ... }  │ │
/// └──────────────────────────────────────────────────────────────┘ │
///                                                                   │
///   (4) Receive via crossbeam::select! on two channels: ───────────┘
///       - rx: Control messages (PrefetchProofs, StateUpdate,
///             EmptyProof, FinishedStateUpdates)
///       - proof_result_rx: Computed proof results from workers
/// ```
///
/// ## Component Responsibilities
///
/// - **[`MultiProofTask`]**: Event loop coordinator
///   - Receives state updates from transaction execution
///   - Deduplicates proof targets against already-fetched proofs
///   - Sequences proofs to maintain transaction ordering
///   - Feeds sequenced updates to sparse trie task
///
/// - **[`MultiproofManager`]**: Calculation orchestrator
///   - Decides between fast path ([`EmptyProof`]) and worker dispatch
///   - Routes storage-only vs full multiproofs to appropriate workers
///   - Records metrics for monitoring
///
/// - **[`ProofWorkerHandle`]**: Worker pool manager
///   - Maintains separate pools for storage and account proofs
///   - Dispatches work to blocking threads (CPU-intensive)
///   - Sends results directly via `proof_result_tx` (bypasses control channel)
///
/// [`EmptyProof`]: MultiProofMessage::EmptyProof
/// [`ProofWorkerHandle`]: reth_trie_parallel::proof_task::ProofWorkerHandle
///
/// ## Dual-Channel Design Rationale
///
/// The system uses two separate crossbeam channels:
///
/// 1. **Control Channel (`tx`/`rx`)**: For orchestration messages
///    - `PrefetchProofs`: Pre-fetch proofs before execution
///    - `StateUpdate`: New transaction execution results
///    - `EmptyProof`: Fast path when all targets already fetched
///    - `FinishedStateUpdates`: Signal to drain pending work
///
/// 2. **Proof Result Channel (`proof_result_tx`/`proof_result_rx`)**: For worker results
///    - `ProofResultMessage`: Computed multiproofs from worker pools
///    - Direct path from workers to event loop (no intermediate hops)
///    - Keeps control messages separate from high-throughput proof data
///
/// This separation enables:
/// - **Non-blocking control**: Control messages never wait behind large proof data
/// - **Backpressure management**: Each channel can apply different policies
/// - **Clear ownership**: Workers only need proof result sender, not control channel
///
/// ## Initialization and Lifecycle
///
/// The task initializes a blinded sparse trie and subscribes to transaction state streams.
/// As it receives transaction execution results, it fetches proofs for relevant accounts
/// from the database and reveals them to the tree, then updates relevant leaves according
/// to transaction results. This feeds updates to the sparse trie task.
///
/// See the `run()` method documentation for detailed lifecycle flow.
#[derive(Debug)]
pub(super) struct MultiProofTask {
    /// The size of proof targets chunk to spawn in one calculation.
    /// If None, chunking is disabled and all targets are processed in a single proof.
    chunk_size: Option<usize>,
    /// Receiver for state root related messages (prefetch, state updates, finish signal).
    rx: CrossbeamReceiver<MultiProofMessage>,
    /// Sender for state root related messages.
    tx: CrossbeamSender<MultiProofMessage>,
    /// Receiver for proof results directly from workers.
    proof_result_rx: CrossbeamReceiver<ProofResultMessage>,
    /// Sender for state updates emitted by this type.
    to_sparse_trie: std::sync::mpsc::Sender<SparseTrieUpdate>,
    /// Proof targets that have been already fetched.
    fetched_proof_targets: MultiProofTargets,
    /// Tracks keys which have been added and removed throughout the entire block.
    multi_added_removed_keys: MultiAddedRemovedKeys,
    /// Proof sequencing handler.
    proof_sequencer: ProofSequencer,
    /// Manages calculation of multiproofs.
    multiproof_manager: MultiproofManager,
    /// multi proof task metrics
    metrics: MultiProofTaskMetrics,
    /// If this number is exceeded and chunking is enabled, then this will override whether or not
    /// there are any active workers and force chunking across workers. This is to prevent tasks
    /// which are very long from hitting a single worker.
    max_targets_for_chunking: usize,
}

impl MultiProofTask {
    /// Creates a multiproof task with separate channels: control on `tx`/`rx`, proof results on
    /// `proof_result_rx`.
    pub(super) fn new(
        proof_worker_handle: ProofWorkerHandle,
        to_sparse_trie: std::sync::mpsc::Sender<SparseTrieUpdate>,
        chunk_size: Option<usize>,
        tx: CrossbeamSender<MultiProofMessage>,
        rx: CrossbeamReceiver<MultiProofMessage>,
    ) -> Self {
        let (proof_result_tx, proof_result_rx) = unbounded();
        let metrics = MultiProofTaskMetrics::default();

        Self {
            chunk_size,
            rx,
            tx,
            proof_result_rx,
            to_sparse_trie,
            fetched_proof_targets: Default::default(),
            multi_added_removed_keys: MultiAddedRemovedKeys::new(),
            proof_sequencer: ProofSequencer::default(),
            multiproof_manager: MultiproofManager::new(
                metrics.clone(),
                proof_worker_handle,
                proof_result_tx,
            ),
            metrics,
            max_targets_for_chunking: DEFAULT_MAX_TARGETS_FOR_CHUNKING,
        }
    }

    /// Handles request for proof prefetch.
    ///
    /// Returns how many multiproof tasks were dispatched for the prefetch request.
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor::multiproof",
        skip_all,
        fields(accounts = targets.len(), chunks = 0)
    )]
    fn on_prefetch_proof(&mut self, mut targets: MultiProofTargets) -> u64 {
        // Remove already fetched proof targets to avoid redundant work.
        targets.retain_difference(&self.fetched_proof_targets);
        self.fetched_proof_targets.extend_ref(&targets);

        // Make sure all target accounts have an `AddedRemovedKeySet` in the
        // [`MultiAddedRemovedKeys`]. Even if there are not any known removed keys for the account,
        // we still want to optimistically fetch extension children for the leaf addition case.
        self.multi_added_removed_keys.touch_accounts(targets.keys().copied());

        // Clone+Arc MultiAddedRemovedKeys for sharing with the dispatched multiproof tasks
        let multi_added_removed_keys = Arc::new(self.multi_added_removed_keys.clone());

        self.metrics.prefetch_proof_targets_accounts_histogram.record(targets.len() as f64);
        self.metrics
            .prefetch_proof_targets_storages_histogram
            .record(targets.values().map(|slots| slots.len()).sum::<usize>() as f64);

        let chunking_len = targets.chunking_length();
        let available_account_workers =
            self.multiproof_manager.proof_worker_handle.available_account_workers();
        let available_storage_workers =
            self.multiproof_manager.proof_worker_handle.available_storage_workers();
        let num_chunks = dispatch_with_chunking(
            targets,
            chunking_len,
            self.chunk_size,
            self.max_targets_for_chunking,
            available_account_workers,
            available_storage_workers,
            MultiProofTargets::chunks,
            |proof_targets| {
                self.multiproof_manager.dispatch(MultiproofInput {
                    source: None,
                    hashed_state_update: Default::default(),
                    proof_targets,
                    proof_sequence_number: self.proof_sequencer.next_sequence(),
                    state_root_message_sender: self.tx.clone(),
                    multi_added_removed_keys: Some(multi_added_removed_keys.clone()),
                });
            },
        );
        self.metrics.prefetch_proof_chunks_histogram.record(num_chunks as f64);

        num_chunks as u64
    }

    // Returns true if all state updates finished and all proofs processed.
    fn is_done(
        &self,
        proofs_processed: u64,
        state_update_proofs_requested: u64,
        prefetch_proofs_requested: u64,
        updates_finished: bool,
    ) -> bool {
        let all_proofs_processed =
            proofs_processed >= state_update_proofs_requested + prefetch_proofs_requested;
        let no_pending = !self.proof_sequencer.has_pending();
        trace!(
            target: "engine::tree::payload_processor::multiproof",
            proofs_processed,
            state_update_proofs_requested,
            prefetch_proofs_requested,
            no_pending,
            updates_finished,
            "Checking end condition"
        );
        all_proofs_processed && no_pending && updates_finished
    }

    /// Handles state updates.
    ///
    /// Returns how many proof dispatches were spawned (including an `EmptyProof` for already
    /// fetched targets).
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_processor::multiproof",
        skip(self, update),
        fields(accounts = update.len(), chunks = 0)
    )]
    fn on_state_update(&mut self, source: Source, update: EvmState) -> u64 {
        let hashed_state_update = evm_state_to_hashed_post_state(update);
        self.on_hashed_state_update(source, hashed_state_update)
    }

    /// Processes a hashed state update and dispatches multiproofs as needed.
    ///
    /// Returns the number of state updates dispatched (both `EmptyProof` and regular multiproofs).
    fn on_hashed_state_update(
        &mut self,
        source: Source,
        hashed_state_update: HashedPostState,
    ) -> u64 {
        // Update removed keys based on the state update.
        self.multi_added_removed_keys.update_with_state(&hashed_state_update);

        // Split the state update into already fetched and not fetched according to the proof
        // targets.
        let (fetched_state_update, not_fetched_state_update) = hashed_state_update
            .partition_by_targets(&self.fetched_proof_targets, &self.multi_added_removed_keys);

        let mut state_updates = 0;
        // If there are any accounts or storage slots that we already fetched the proofs for,
        // send them immediately, as they don't require dispatching any additional multiproofs.
        if !fetched_state_update.is_empty() {
            let _ = self.tx.send(MultiProofMessage::EmptyProof {
                sequence_number: self.proof_sequencer.next_sequence(),
                state: fetched_state_update,
            });
            state_updates += 1;
        }

        // Clone+Arc MultiAddedRemovedKeys for sharing with the dispatched multiproof tasks
        let multi_added_removed_keys = Arc::new(self.multi_added_removed_keys.clone());

        let chunking_len = not_fetched_state_update.chunking_length();
        let mut spawned_proof_targets = MultiProofTargets::default();
        let available_account_workers =
            self.multiproof_manager.proof_worker_handle.available_account_workers();
        let available_storage_workers =
            self.multiproof_manager.proof_worker_handle.available_storage_workers();
        let num_chunks = dispatch_with_chunking(
            not_fetched_state_update,
            chunking_len,
            self.chunk_size,
            self.max_targets_for_chunking,
            available_account_workers,
            available_storage_workers,
            HashedPostState::chunks,
            |hashed_state_update| {
                let proof_targets = get_proof_targets(
                    &hashed_state_update,
                    &self.fetched_proof_targets,
                    &multi_added_removed_keys,
                );
                spawned_proof_targets.extend_ref(&proof_targets);

                self.multiproof_manager.dispatch(MultiproofInput {
                    source: Some(source),
                    hashed_state_update,
                    proof_targets,
                    proof_sequence_number: self.proof_sequencer.next_sequence(),
                    state_root_message_sender: self.tx.clone(),
                    multi_added_removed_keys: Some(multi_added_removed_keys.clone()),
                });
            },
        );
        self.metrics
            .state_update_proof_targets_accounts_histogram
            .record(spawned_proof_targets.len() as f64);
        self.metrics
            .state_update_proof_targets_storages_histogram
            .record(spawned_proof_targets.values().map(|slots| slots.len()).sum::<usize>() as f64);
        self.metrics.state_update_proof_chunks_histogram.record(num_chunks as f64);

        self.fetched_proof_targets.extend(spawned_proof_targets);

        state_updates + num_chunks as u64
    }

    /// Handler for new proof calculated, aggregates all the existing sequential proofs.
    fn on_proof(
        &mut self,
        sequence_number: u64,
        update: SparseTrieUpdate,
    ) -> Option<SparseTrieUpdate> {
        let ready_proofs = self.proof_sequencer.add_proof(sequence_number, update);

        ready_proofs
            .into_iter()
            // Merge all ready proofs and state updates
            .reduce(|mut acc_update, update| {
                acc_update.extend(update);
                acc_update
            })
            // Return None if the resulting proof is empty
            .filter(|proof| !proof.is_empty())
    }

    /// Processes a multiproof message, batching consecutive prefetch messages.
    ///
    /// For prefetch messages, drains queued prefetch messages and merges them into one batch before
    /// processing, storing one pending message (different type or over-cap) to handle on the next
    /// iteration. State updates are processed directly without batching.
    ///
    /// Returns `true` if done, `false` to continue.
    fn process_multiproof_message<P>(
        &mut self,
        msg: MultiProofMessage,
        ctx: &mut MultiproofBatchCtx,
        batch_metrics: &mut MultiproofBatchMetrics,
        provider: &P,
    ) -> bool
    where
        P: AccountReader,
    {
        match msg {
            // Prefetch proofs: batch consecutive prefetch requests up to target/message limits
            MultiProofMessage::PrefetchProofs(targets) => {
                trace!(target: "engine::tree::payload_processor::multiproof", "processing MultiProofMessage::PrefetchProofs");

                if ctx.first_update_time.is_none() {
                    self.metrics
                        .first_update_wait_time_histogram
                        .record(ctx.start.elapsed().as_secs_f64());
                    ctx.first_update_time = Some(Instant::now());
                    debug!(target: "engine::tree::payload_processor::multiproof", "Started state root calculation");
                }

                let mut accumulated_count = targets.chunking_length();
                ctx.accumulated_prefetch_targets.clear();
                ctx.accumulated_prefetch_targets.push(targets);

                // Batch consecutive prefetch messages up to limits.
                // EmptyProof messages are handled inline since they're very fast (~100ns)
                // and shouldn't interrupt batching.
                while accumulated_count < PREFETCH_MAX_BATCH_TARGETS &&
                    ctx.accumulated_prefetch_targets.len() < PREFETCH_MAX_BATCH_MESSAGES
                {
                    match self.rx.try_recv() {
                        Ok(MultiProofMessage::PrefetchProofs(next_targets)) => {
                            let next_count = next_targets.chunking_length();
                            if accumulated_count + next_count > PREFETCH_MAX_BATCH_TARGETS {
                                ctx.pending_msg =
                                    Some(MultiProofMessage::PrefetchProofs(next_targets));
                                break;
                            }
                            accumulated_count += next_count;
                            ctx.accumulated_prefetch_targets.push(next_targets);
                        }
                        Ok(MultiProofMessage::EmptyProof { sequence_number, state }) => {
                            // Handle inline - very fast, don't break batching
                            batch_metrics.proofs_processed += 1;
                            if let Some(combined_update) = self.on_proof(
                                sequence_number,
                                SparseTrieUpdate { state, multiproof: Default::default() },
                            ) {
                                let _ = self.to_sparse_trie.send(combined_update);
                            }
                        }
                        Ok(other_msg) => {
                            ctx.pending_msg = Some(other_msg);
                            break;
                        }
                        Err(_) => break,
                    }
                }

                // Process all accumulated messages in a single batch
                let num_batched = ctx.accumulated_prefetch_targets.len();
                self.metrics.prefetch_batch_size_histogram.record(num_batched as f64);

                // Merge all accumulated prefetch targets into a single dispatch payload.
                // Use drain to preserve the buffer allocation.
                let mut accumulated_iter = ctx.accumulated_prefetch_targets.drain(..);
                let mut merged_targets =
                    accumulated_iter.next().expect("prefetch batch always has at least one entry");
                for next_targets in accumulated_iter {
                    merged_targets.extend(next_targets);
                }

                let account_targets = merged_targets.len();
                let storage_targets =
                    merged_targets.values().map(|slots| slots.len()).sum::<usize>();
                batch_metrics.prefetch_proofs_requested += self.on_prefetch_proof(merged_targets);
                trace!(
                    target: "engine::tree::payload_processor::multiproof",
                    account_targets,
                    storage_targets,
                    prefetch_proofs_requested = batch_metrics.prefetch_proofs_requested,
                    num_batched,
                    "Dispatched prefetch batch"
                );

                false
            }
            MultiProofMessage::StateUpdate(source, update) => {
                trace!(target: "engine::tree::payload_processor::multiproof", "processing MultiProofMessage::StateUpdate");

                if ctx.first_update_time.is_none() {
                    self.metrics
                        .first_update_wait_time_histogram
                        .record(ctx.start.elapsed().as_secs_f64());
                    ctx.first_update_time = Some(Instant::now());
                    debug!(target: "engine::tree::payload_processor::multiproof", "Started state root calculation");
                }

                let update_len = update.len();
                batch_metrics.state_update_proofs_requested += self.on_state_update(source, update);
                trace!(
                    target: "engine::tree::payload_processor::multiproof",
                    ?source,
                    len = update_len,
                    state_update_proofs_requested = ?batch_metrics.state_update_proofs_requested,
                    "Dispatched state update"
                );

                false
            }
            // Process Block Access List (BAL) - complete state changes provided upfront
            MultiProofMessage::BlockAccessList(bal) => {
                trace!(target: "engine::tree::payload_processor::multiproof", "processing MultiProofMessage::BAL");

                if ctx.first_update_time.is_none() {
                    self.metrics
                        .first_update_wait_time_histogram
                        .record(ctx.start.elapsed().as_secs_f64());
                    ctx.first_update_time = Some(Instant::now());
                    debug!(target: "engine::tree::payload_processor::multiproof", "Started state root calculation from BAL");
                }

                // Convert BAL to HashedPostState and process it
                match bal_to_hashed_post_state(&bal, provider) {
                    Ok(hashed_state) => {
                        debug!(
                            target: "engine::tree::payload_processor::multiproof",
                            accounts = hashed_state.accounts.len(),
                            storages = hashed_state.storages.len(),
                            "Processing BAL state update"
                        );

                        // Use BlockAccessList as source for BAL-derived state updates
                        batch_metrics.state_update_proofs_requested +=
                            self.on_hashed_state_update(Source::BlockAccessList, hashed_state);
                    }
                    Err(err) => {
                        error!(target: "engine::tree::payload_processor::multiproof", ?err, "Failed to convert BAL to hashed state");
                        return true;
                    }
                }

                // Mark updates as finished since BAL provides complete state
                ctx.updates_finished_time = Some(Instant::now());

                // Check if we're done (might need to wait for proofs to complete)
                if self.is_done(
                    batch_metrics.proofs_processed,
                    batch_metrics.state_update_proofs_requested,
                    batch_metrics.prefetch_proofs_requested,
                    ctx.updates_finished(),
                ) {
                    debug!(
                        target: "engine::tree::payload_processor::multiproof",
                        "BAL processed and all proofs complete, ending calculation"
                    );
                    return true;
                }
                false
            }
            // Signal that no more state updates will arrive
            MultiProofMessage::FinishedStateUpdates => {
                trace!(target: "engine::tree::payload_processor::multiproof", "processing MultiProofMessage::FinishedStateUpdates");

                ctx.updates_finished_time = Some(Instant::now());

                if self.is_done(
                    batch_metrics.proofs_processed,
                    batch_metrics.state_update_proofs_requested,
                    batch_metrics.prefetch_proofs_requested,
                    ctx.updates_finished(),
                ) {
                    debug!(
                        target: "engine::tree::payload_processor::multiproof",
                        "State updates finished and all proofs processed, ending calculation"
                    );
                    return true;
                }
                false
            }
            // Handle proof result with no trie nodes (state unchanged)
            MultiProofMessage::EmptyProof { sequence_number, state } => {
                trace!(target: "engine::tree::payload_processor::multiproof", "processing MultiProofMessage::EmptyProof");

                batch_metrics.proofs_processed += 1;

                if let Some(combined_update) = self.on_proof(
                    sequence_number,
                    SparseTrieUpdate { state, multiproof: Default::default() },
                ) {
                    let _ = self.to_sparse_trie.send(combined_update);
                }

                if self.is_done(
                    batch_metrics.proofs_processed,
                    batch_metrics.state_update_proofs_requested,
                    batch_metrics.prefetch_proofs_requested,
                    ctx.updates_finished(),
                ) {
                    debug!(
                        target: "engine::tree::payload_processor::multiproof",
                        "State updates finished and all proofs processed, ending calculation"
                    );
                    return true;
                }
                false
            }
        }
    }

    /// Starts the main loop that handles all incoming messages, fetches proofs, applies them to the
    /// sparse trie, updates the sparse trie, and eventually returns the state root.
    ///
    /// The lifecycle is the following:
    /// 1. Either [`MultiProofMessage::PrefetchProofs`] or [`MultiProofMessage::StateUpdate`] is
    ///    received from the engine.
    ///    * For [`MultiProofMessage::StateUpdate`], the state update is hashed with
    ///      [`evm_state_to_hashed_post_state`], and then (proof targets)[`MultiProofTargets`] are
    ///      extracted with [`get_proof_targets`].
    ///    * For both messages, proof targets are deduplicated according to `fetched_proof_targets`,
    ///      so that the proofs for accounts and storage slots that were already fetched are not
    ///      requested again.
    /// 2. Using the proof targets, a new multiproof is calculated using
    ///    [`MultiproofManager::dispatch`].
    ///    * If the list of proof targets is empty, the [`MultiProofMessage::EmptyProof`] message is
    ///      sent back to this task along with the original state update.
    ///    * Otherwise, the multiproof is dispatched to worker pools and results are sent directly
    ///      to this task via the `proof_result_rx` channel as [`ProofResultMessage`].
    /// 3. Either [`MultiProofMessage::EmptyProof`] (via control channel) or [`ProofResultMessage`]
    ///    (via proof result channel) is received.
    ///    * The multiproof is added to the [`ProofSequencer`].
    ///    * If the proof sequencer has a contiguous sequence of multiproofs in the same order as
    ///      state updates arrived (i.e. transaction order), such sequence is returned.
    /// 4. Once there's a sequence of contiguous multiproofs along with the proof targets and state
    ///    updates associated with them, a [`SparseTrieUpdate`] is generated and sent to the sparse
    ///    trie task.
    /// 5. Steps above are repeated until this task receives a
    ///    [`MultiProofMessage::FinishedStateUpdates`].
    ///    * Once this message is received, on every [`MultiProofMessage::EmptyProof`] and
    ///      [`ProofResultMessage`], we check if all proofs have been processed and if there are any
    ///      pending proofs in the proof sequencer left to be revealed.
    /// 6. While running, consecutive [`MultiProofMessage::PrefetchProofs`] messages are batched to
    ///    reduce redundant work; if a different message type arrives mid-batch or a batch cap is
    ///    reached, it is held as `pending_msg` and processed on the next loop to preserve ordering.
    /// 7. This task exits after all pending proofs are processed.
    #[instrument(
        level = "debug",
        name = "MultiProofTask::run",
        target = "engine::tree::payload_processor::multiproof",
        skip_all
    )]
    pub(crate) fn run<P>(mut self, provider: P)
    where
        P: AccountReader,
    {
        let mut ctx = MultiproofBatchCtx::new(Instant::now());
        let mut batch_metrics = MultiproofBatchMetrics::default();

        // Main event loop; select_biased! prioritizes proof results over control messages.
        // Labeled so inner match arms can `break 'main` once all work is complete.
        'main: loop {
            trace!(target: "engine::tree::payload_processor::multiproof", "entering main channel receiving loop");

            if let Some(msg) = ctx.pending_msg.take() {
                if self.process_multiproof_message(msg, &mut ctx, &mut batch_metrics, &provider) {
                    break 'main;
                }
                continue;
            }

            // Use select_biased! to prioritize proof results over new requests.
            // This prevents new work from starving completed proofs and keeps workers healthy.
            crossbeam_channel::select_biased! {
                recv(self.proof_result_rx) -> proof_msg => {
                    match proof_msg {
                        Ok(proof_result) => {
                            batch_metrics.proofs_processed += 1;

                            self.metrics
                                .proof_calculation_duration_histogram
                                .record(proof_result.elapsed);

                            self.multiproof_manager.on_calculation_complete();

                            // Convert ProofResultMessage to SparseTrieUpdate
                            match proof_result.result {
                                Ok(proof_result_data) => {
                                    trace!(
                                        target: "engine::tree::payload_processor::multiproof",
                                        sequence = proof_result.sequence_number,
                                        total_proofs = batch_metrics.proofs_processed,
                                        "Processing calculated proof from worker"
                                    );

                                    let update = SparseTrieUpdate {
                                        state: proof_result.state,
                                        multiproof: proof_result_data.proof,
                                    };

                                    if let Some(combined_update) =
                                        self.on_proof(proof_result.sequence_number, update)
                                    {
                                        let _ = self.to_sparse_trie.send(combined_update);
                                    }
                                }
                                Err(error) => {
                                    error!(target: "engine::tree::payload_processor::multiproof", ?error, "proof calculation error from worker");
                                    return
                                }
                            }

                            if self.is_done(
                                batch_metrics.proofs_processed,
                                batch_metrics.state_update_proofs_requested,
                                batch_metrics.prefetch_proofs_requested,
                                ctx.updates_finished(),
                            ) {
                                debug!(
                                    target: "engine::tree::payload_processor::multiproof",
                                    "State updates finished and all proofs processed, ending calculation"
                                );
                                break 'main
                            }
                        }
                        Err(_) => {
                            error!(target: "engine::tree::payload_processor::multiproof", "Proof result channel closed unexpectedly");
                            return
                        }
                    }
                },
                recv(self.rx) -> message => {
                    let msg = match message {
                        Ok(m) => m,
                        Err(_) => {
                            error!(target: "engine::tree::payload_processor::multiproof", "State root related message channel closed unexpectedly");
                            return
                        }
                    };

                    if self.process_multiproof_message(msg, &mut ctx, &mut batch_metrics, &provider) {
                        break 'main;
                    }
                }
            }
        }

        debug!(
            target: "engine::tree::payload_processor::multiproof",
            total_updates = batch_metrics.state_update_proofs_requested,
            total_proofs = batch_metrics.proofs_processed,
            total_time = ?ctx.first_update_time.map(|t|t.elapsed()),
            time_since_updates_finished = ?ctx.updates_finished_time.map(|t|t.elapsed()),
            "All proofs processed, ending calculation"
        );

        // update total metrics on finish
        self.metrics
            .state_updates_received_histogram
            .record(batch_metrics.state_update_proofs_requested as f64);
        self.metrics.proofs_processed_histogram.record(batch_metrics.proofs_processed as f64);
        if let Some(total_time) = ctx.first_update_time.map(|t| t.elapsed()) {
            self.metrics.multiproof_task_total_duration_histogram.record(total_time);
        }

        if let Some(updates_finished_time) = ctx.updates_finished_time {
            self.metrics
                .last_proof_wait_time_histogram
                .record(updates_finished_time.elapsed().as_secs_f64());
        }
    }
}

/// Context for multiproof message batching loop.
///
/// Contains processing state that persists across loop iterations.
///
/// Used by `process_multiproof_message` to batch consecutive prefetch messages received via
/// `try_recv` for efficient processing.
struct MultiproofBatchCtx {
    /// Buffers a non-matching message type encountered during batching.
    /// Processed first in next iteration to preserve ordering while allowing same-type
    /// messages to batch.
    pending_msg: Option<MultiProofMessage>,
    /// Timestamp when the first state update or prefetch was received.
    first_update_time: Option<Instant>,
    /// Timestamp before the first state update or prefetch was received.
    start: Instant,
    /// Timestamp when state updates finished. `Some` indicates all state updates have been
    /// received.
    updates_finished_time: Option<Instant>,
    /// Reusable buffer for accumulating prefetch targets during batching.
    accumulated_prefetch_targets: Vec<MultiProofTargets>,
}

impl MultiproofBatchCtx {
    /// Creates a new batch context with the given start time.
    fn new(start: Instant) -> Self {
        Self {
            pending_msg: None,
            first_update_time: None,
            start,
            updates_finished_time: None,
            accumulated_prefetch_targets: Vec::with_capacity(PREFETCH_MAX_BATCH_MESSAGES),
        }
    }

    /// Returns `true` if all state updates have been received.
    const fn updates_finished(&self) -> bool {
        self.updates_finished_time.is_some()
    }
}

/// Counters for tracking proof requests and processing.
#[derive(Default)]
struct MultiproofBatchMetrics {
    /// Number of proofs that have been processed.
    proofs_processed: u64,
    /// Number of state update proofs requested.
    state_update_proofs_requested: u64,
    /// Number of prefetch proofs requested.
    prefetch_proofs_requested: u64,
}

/// Returns accounts only with those storages that were not already fetched, and
/// if there are no such storages and the account itself was already fetched, the
/// account shouldn't be included.
fn get_proof_targets(
    state_update: &HashedPostState,
    fetched_proof_targets: &MultiProofTargets,
    multi_added_removed_keys: &MultiAddedRemovedKeys,
) -> MultiProofTargets {
    let mut targets = MultiProofTargets::default();

    // first collect all new accounts (not previously fetched)
    for hashed_address in state_update.accounts.keys() {
        if !fetched_proof_targets.contains_key(hashed_address) {
            targets.insert(*hashed_address, HashSet::default());
        }
    }

    // then process storage slots for all accounts in the state update
    for (hashed_address, storage) in &state_update.storages {
        let fetched = fetched_proof_targets.get(hashed_address);
        let storage_added_removed_keys = multi_added_removed_keys.get_storage(hashed_address);
        let mut changed_slots = storage
            .storage
            .keys()
            .filter(|slot| {
                !fetched.is_some_and(|f| f.contains(*slot)) ||
                    storage_added_removed_keys.is_some_and(|k| k.is_removed(slot))
            })
            .peekable();

        // If the storage is wiped, we still need to fetch the account proof.
        if storage.wiped && fetched.is_none() {
            targets.entry(*hashed_address).or_default();
        }

        if changed_slots.peek().is_some() {
            targets.entry(*hashed_address).or_default().extend(changed_slots);
        }
    }

    targets
}

/// Dispatches work items as a single unit or in chunks based on target size and worker
/// availability.
#[allow(clippy::too_many_arguments)]
fn dispatch_with_chunking<T, I>(
    items: T,
    chunking_len: usize,
    chunk_size: Option<usize>,
    max_targets_for_chunking: usize,
    available_account_workers: usize,
    available_storage_workers: usize,
    chunker: impl FnOnce(T, usize) -> I,
    mut dispatch: impl FnMut(T),
) -> usize
where
    I: IntoIterator<Item = T>,
{
    let should_chunk = chunking_len > max_targets_for_chunking ||
        available_account_workers > 1 ||
        available_storage_workers > 1;

    if should_chunk &&
        let Some(chunk_size) = chunk_size &&
        chunking_len > chunk_size
    {
        let mut num_chunks = 0usize;
        for chunk in chunker(items, chunk_size) {
            dispatch(chunk);
            num_chunks += 1;
        }
        return num_chunks;
    }

    dispatch(items);
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::cached_state::{CachedStateProvider, ExecutionCacheBuilder};
    use alloy_eip7928::{AccountChanges, BalanceChange};
    use alloy_primitives::Address;
    use reth_provider::{
        providers::OverlayStateProviderFactory, test_utils::create_test_provider_factory,
        BlockNumReader, BlockReader, ChangeSetReader, DatabaseProviderFactory, LatestStateProvider,
        PruneCheckpointReader, StageCheckpointReader, StateProviderBox, StorageChangeSetReader,
        TrieReader,
    };
    use reth_trie::MultiProof;
    use reth_trie_parallel::proof_task::{ProofTaskCtx, ProofWorkerHandle};
    use revm_primitives::{B256, U256};
    use std::sync::{Arc, OnceLock};
    use tokio::runtime::{Handle, Runtime};

    /// Get a handle to the test runtime, creating it if necessary
    fn get_test_runtime_handle() -> Handle {
        static TEST_RT: OnceLock<Runtime> = OnceLock::new();
        TEST_RT
            .get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap()
            })
            .handle()
            .clone()
    }

    fn create_test_state_root_task<F>(factory: F) -> MultiProofTask
    where
        F: DatabaseProviderFactory<
                Provider: BlockReader
                              + TrieReader
                              + StageCheckpointReader
                              + PruneCheckpointReader
                              + ChangeSetReader
                              + StorageChangeSetReader
                              + BlockNumReader,
            > + Clone
            + Send
            + 'static,
    {
        let rt_handle = get_test_runtime_handle();
        let overlay_factory = OverlayStateProviderFactory::new(factory);
        let task_ctx = ProofTaskCtx::new(overlay_factory);
        let proof_handle = ProofWorkerHandle::new(rt_handle, task_ctx, 1, 1, false);
        let (to_sparse_trie, _receiver) = std::sync::mpsc::channel();
        let (tx, rx) = crossbeam_channel::unbounded();

        MultiProofTask::new(proof_handle, to_sparse_trie, Some(1), tx, rx)
    }

    fn create_cached_provider<F>(factory: F) -> CachedStateProvider<StateProviderBox>
    where
        F: DatabaseProviderFactory<
                Provider: BlockReader + TrieReader + StageCheckpointReader + PruneCheckpointReader,
            > + Clone
            + Send
            + 'static,
    {
        let db_provider = factory.database_provider_ro().unwrap();
        let state_provider: StateProviderBox = Box::new(LatestStateProvider::new(db_provider));
        let cache = ExecutionCacheBuilder::default().build_caches(1000);
        CachedStateProvider::new(state_provider, cache, Default::default())
    }

    #[test]
    fn test_add_proof_in_sequence() {
        let mut sequencer = ProofSequencer::default();
        let proof1 = MultiProof::default();
        let proof2 = MultiProof::default();
        sequencer.next_sequence = 2;

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof1).unwrap());
        assert_eq!(ready.len(), 1);
        assert!(!sequencer.has_pending());

        let ready = sequencer.add_proof(1, SparseTrieUpdate::from_multiproof(proof2).unwrap());
        assert_eq!(ready.len(), 1);
        assert!(!sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_out_of_order() {
        let mut sequencer = ProofSequencer::default();
        let proof1 = MultiProof::default();
        let proof2 = MultiProof::default();
        let proof3 = MultiProof::default();
        sequencer.next_sequence = 3;

        let ready = sequencer.add_proof(2, SparseTrieUpdate::from_multiproof(proof3).unwrap());
        assert_eq!(ready.len(), 0);
        assert!(sequencer.has_pending());

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof1).unwrap());
        assert_eq!(ready.len(), 1);
        assert!(sequencer.has_pending());

        let ready = sequencer.add_proof(1, SparseTrieUpdate::from_multiproof(proof2).unwrap());
        assert_eq!(ready.len(), 2);
        assert!(!sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_with_gaps() {
        let mut sequencer = ProofSequencer::default();
        let proof1 = MultiProof::default();
        let proof3 = MultiProof::default();
        sequencer.next_sequence = 3;

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof1).unwrap());
        assert_eq!(ready.len(), 1);

        let ready = sequencer.add_proof(2, SparseTrieUpdate::from_multiproof(proof3).unwrap());
        assert_eq!(ready.len(), 0);
        assert!(sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_duplicate_sequence() {
        let mut sequencer = ProofSequencer::default();
        let proof1 = MultiProof::default();
        let proof2 = MultiProof::default();

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof1).unwrap());
        assert_eq!(ready.len(), 1);

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof2).unwrap());
        assert_eq!(ready.len(), 0);
        assert!(!sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_batch_processing() {
        let mut sequencer = ProofSequencer::default();
        let proofs: Vec<_> = (0..5).map(|_| MultiProof::default()).collect();
        sequencer.next_sequence = 5;

        sequencer.add_proof(4, SparseTrieUpdate::from_multiproof(proofs[4].clone()).unwrap());
        sequencer.add_proof(2, SparseTrieUpdate::from_multiproof(proofs[2].clone()).unwrap());
        sequencer.add_proof(1, SparseTrieUpdate::from_multiproof(proofs[1].clone()).unwrap());
        sequencer.add_proof(3, SparseTrieUpdate::from_multiproof(proofs[3].clone()).unwrap());

        let ready =
            sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proofs[0].clone()).unwrap());
        assert_eq!(ready.len(), 5);
        assert!(!sequencer.has_pending());
    }

    fn create_get_proof_targets_state() -> HashedPostState {
        let mut state = HashedPostState::default();

        let addr1 = B256::random();
        let addr2 = B256::random();
        state.accounts.insert(addr1, Some(Default::default()));
        state.accounts.insert(addr2, Some(Default::default()));

        let mut storage = HashedStorage::default();
        let slot1 = B256::random();
        let slot2 = B256::random();
        storage.storage.insert(slot1, U256::ZERO);
        storage.storage.insert(slot2, U256::from(1));
        state.storages.insert(addr1, storage);

        state
    }

    #[test]
    fn test_get_proof_targets_new_account_targets() {
        let state = create_get_proof_targets_state();
        let fetched = MultiProofTargets::default();

        let targets = get_proof_targets(&state, &fetched, &MultiAddedRemovedKeys::new());

        // should return all accounts as targets since nothing was fetched before
        assert_eq!(targets.len(), state.accounts.len());
        for addr in state.accounts.keys() {
            assert!(targets.contains_key(addr));
        }
    }

    #[test]
    fn test_get_proof_targets_new_storage_targets() {
        let state = create_get_proof_targets_state();
        let fetched = MultiProofTargets::default();

        let targets = get_proof_targets(&state, &fetched, &MultiAddedRemovedKeys::new());

        // verify storage slots are included for accounts with storage
        for (addr, storage) in &state.storages {
            assert!(targets.contains_key(addr));
            let target_slots = &targets[addr];
            assert_eq!(target_slots.len(), storage.storage.len());
            for slot in storage.storage.keys() {
                assert!(target_slots.contains(slot));
            }
        }
    }

    #[test]
    fn test_get_proof_targets_filter_already_fetched_accounts() {
        let state = create_get_proof_targets_state();
        let mut fetched = MultiProofTargets::default();

        // select an account that has no storage updates
        let fetched_addr = state
            .accounts
            .keys()
            .find(|&&addr| !state.storages.contains_key(&addr))
            .expect("Should have an account without storage");

        // mark the account as already fetched
        fetched.insert(*fetched_addr, HashSet::default());

        let targets = get_proof_targets(&state, &fetched, &MultiAddedRemovedKeys::new());

        // should not include the already fetched account since it has no storage updates
        assert!(!targets.contains_key(fetched_addr));
        // other accounts should still be included
        assert_eq!(targets.len(), state.accounts.len() - 1);
    }

    #[test]
    fn test_get_proof_targets_filter_already_fetched_storage() {
        let state = create_get_proof_targets_state();
        let mut fetched = MultiProofTargets::default();

        // mark one storage slot as already fetched
        let (addr, storage) = state.storages.iter().next().unwrap();
        let mut fetched_slots = HashSet::default();
        let fetched_slot = *storage.storage.keys().next().unwrap();
        fetched_slots.insert(fetched_slot);
        fetched.insert(*addr, fetched_slots);

        let targets = get_proof_targets(&state, &fetched, &MultiAddedRemovedKeys::new());

        // should not include the already fetched storage slot
        let target_slots = &targets[addr];
        assert!(!target_slots.contains(&fetched_slot));
        assert_eq!(target_slots.len(), storage.storage.len() - 1);
    }

    #[test]
    fn test_get_proof_targets_empty_state() {
        let state = HashedPostState::default();
        let fetched = MultiProofTargets::default();

        let targets = get_proof_targets(&state, &fetched, &MultiAddedRemovedKeys::new());

        assert!(targets.is_empty());
    }

    #[test]
    fn test_get_proof_targets_mixed_fetched_state() {
        let mut state = HashedPostState::default();
        let mut fetched = MultiProofTargets::default();

        let addr1 = B256::random();
        let addr2 = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();

        state.accounts.insert(addr1, Some(Default::default()));
        state.accounts.insert(addr2, Some(Default::default()));

        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::ZERO);
        storage.storage.insert(slot2, U256::from(1));
        state.storages.insert(addr1, storage);

        let mut fetched_slots = HashSet::default();
        fetched_slots.insert(slot1);
        fetched.insert(addr1, fetched_slots);

        let targets = get_proof_targets(&state, &fetched, &MultiAddedRemovedKeys::new());

        assert!(targets.contains_key(&addr2));
        assert!(!targets[&addr1].contains(&slot1));
        assert!(targets[&addr1].contains(&slot2));
    }

    #[test]
    fn test_get_proof_targets_unmodified_account_with_storage() {
        let mut state = HashedPostState::default();
        let fetched = MultiProofTargets::default();

        let addr = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();

        // don't add the account to state.accounts (simulating unmodified account)
        // but add storage updates for this account
        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::from(1));
        storage.storage.insert(slot2, U256::from(2));
        state.storages.insert(addr, storage);

        assert!(!state.accounts.contains_key(&addr));
        assert!(!fetched.contains_key(&addr));

        let targets = get_proof_targets(&state, &fetched, &MultiAddedRemovedKeys::new());

        // verify that we still get the storage slots for the unmodified account
        assert!(targets.contains_key(&addr));

        let target_slots = &targets[&addr];
        assert_eq!(target_slots.len(), 2);
        assert!(target_slots.contains(&slot1));
        assert!(target_slots.contains(&slot2));
    }

    #[test]
    fn test_get_proof_targets_with_removed_storage_keys() {
        let mut state = HashedPostState::default();
        let mut fetched = MultiProofTargets::default();
        let mut multi_added_removed_keys = MultiAddedRemovedKeys::new();

        let addr = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();

        // add account to state
        state.accounts.insert(addr, Some(Default::default()));

        // add storage updates
        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::from(100));
        storage.storage.insert(slot2, U256::from(200));
        state.storages.insert(addr, storage);

        // mark slot1 as already fetched
        let mut fetched_slots = HashSet::default();
        fetched_slots.insert(slot1);
        fetched.insert(addr, fetched_slots);

        // update multi_added_removed_keys to mark slot1 as removed
        let mut removed_state = HashedPostState::default();
        let mut removed_storage = HashedStorage::default();
        removed_storage.storage.insert(slot1, U256::ZERO); // U256::ZERO marks as removed
        removed_state.storages.insert(addr, removed_storage);
        multi_added_removed_keys.update_with_state(&removed_state);

        let targets = get_proof_targets(&state, &fetched, &multi_added_removed_keys);

        // slot1 should be included despite being fetched, because it's marked as removed
        assert!(targets.contains_key(&addr));
        let target_slots = &targets[&addr];
        assert_eq!(target_slots.len(), 2);
        assert!(target_slots.contains(&slot1)); // included because it's removed
        assert!(target_slots.contains(&slot2)); // included because it's not fetched
    }

    #[test]
    fn test_get_proof_targets_with_wiped_storage() {
        let mut state = HashedPostState::default();
        let fetched = MultiProofTargets::default();
        let multi_added_removed_keys = MultiAddedRemovedKeys::new();

        let addr = B256::random();
        let slot1 = B256::random();

        // add account to state
        state.accounts.insert(addr, Some(Default::default()));

        // add wiped storage
        let mut storage = HashedStorage::new(true);
        storage.storage.insert(slot1, U256::from(100));
        state.storages.insert(addr, storage);

        let targets = get_proof_targets(&state, &fetched, &multi_added_removed_keys);

        // account should be included because storage is wiped and account wasn't fetched
        assert!(targets.contains_key(&addr));
        let target_slots = &targets[&addr];
        assert_eq!(target_slots.len(), 1);
        assert!(target_slots.contains(&slot1));
    }

    #[test]
    fn test_get_proof_targets_removed_keys_not_in_state_update() {
        let mut state = HashedPostState::default();
        let mut fetched = MultiProofTargets::default();
        let mut multi_added_removed_keys = MultiAddedRemovedKeys::new();

        let addr = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();
        let slot3 = B256::random();

        // add account to state
        state.accounts.insert(addr, Some(Default::default()));

        // add storage updates for slot1 and slot2 only
        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::from(100));
        storage.storage.insert(slot2, U256::from(200));
        state.storages.insert(addr, storage);

        // mark all slots as already fetched
        let mut fetched_slots = HashSet::default();
        fetched_slots.insert(slot1);
        fetched_slots.insert(slot2);
        fetched_slots.insert(slot3); // slot3 is fetched but not in state update
        fetched.insert(addr, fetched_slots);

        // mark slot3 as removed (even though it's not in the state update)
        let mut removed_state = HashedPostState::default();
        let mut removed_storage = HashedStorage::default();
        removed_storage.storage.insert(slot3, U256::ZERO);
        removed_state.storages.insert(addr, removed_storage);
        multi_added_removed_keys.update_with_state(&removed_state);

        let targets = get_proof_targets(&state, &fetched, &multi_added_removed_keys);

        // only slots in the state update can be included, so slot3 should not appear
        assert!(!targets.contains_key(&addr));
    }

    /// Verifies that consecutive prefetch proof messages are batched together.
    #[test]
    fn test_prefetch_proofs_batching() {
        let test_provider_factory = create_test_provider_factory();
        let mut task = create_test_state_root_task(test_provider_factory);

        // send multiple messages
        let addr1 = B256::random();
        let addr2 = B256::random();
        let addr3 = B256::random();

        let mut targets1 = MultiProofTargets::default();
        targets1.insert(addr1, HashSet::default());

        let mut targets2 = MultiProofTargets::default();
        targets2.insert(addr2, HashSet::default());

        let mut targets3 = MultiProofTargets::default();
        targets3.insert(addr3, HashSet::default());

        let tx = task.tx.clone();
        tx.send(MultiProofMessage::PrefetchProofs(targets1)).unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(targets2)).unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(targets3)).unwrap();

        let proofs_requested =
            if let Ok(MultiProofMessage::PrefetchProofs(targets)) = task.rx.recv() {
                // simulate the batching logic
                let mut merged_targets = targets;
                let mut num_batched = 1;
                while let Ok(MultiProofMessage::PrefetchProofs(next_targets)) = task.rx.try_recv() {
                    merged_targets.extend(next_targets);
                    num_batched += 1;
                }

                assert_eq!(num_batched, 3);
                assert_eq!(merged_targets.len(), 3);
                assert!(merged_targets.contains_key(&addr1));
                assert!(merged_targets.contains_key(&addr2));
                assert!(merged_targets.contains_key(&addr3));

                task.on_prefetch_proof(merged_targets)
            } else {
                panic!("Expected PrefetchProofs message");
            };

        assert_eq!(proofs_requested, 1);
    }

    /// Verifies that different message types arriving mid-batch are not lost and preserve order.
    #[test]
    fn test_batching_preserves_ordering_with_different_message_type() {
        use alloy_evm::block::StateChangeSource;
        use revm_state::Account;

        let test_provider_factory = create_test_provider_factory();
        let task = create_test_state_root_task(test_provider_factory);

        let addr1 = B256::random();
        let addr2 = B256::random();
        let addr3 = B256::random();
        let state_addr1 = alloy_primitives::Address::random();
        let state_addr2 = alloy_primitives::Address::random();

        // Create PrefetchProofs targets
        let mut targets1 = MultiProofTargets::default();
        targets1.insert(addr1, HashSet::default());

        let mut targets2 = MultiProofTargets::default();
        targets2.insert(addr2, HashSet::default());

        let mut targets3 = MultiProofTargets::default();
        targets3.insert(addr3, HashSet::default());

        // Create StateUpdate 1
        let mut state_update1 = EvmState::default();
        state_update1.insert(
            state_addr1,
            Account {
                info: revm_state::AccountInfo {
                    balance: U256::from(100),
                    nonce: 1,
                    code_hash: Default::default(),
                    code: Default::default(),
                },
                transaction_id: Default::default(),
                storage: Default::default(),
                status: revm_state::AccountStatus::Touched,
            },
        );

        // Create StateUpdate 2
        let mut state_update2 = EvmState::default();
        state_update2.insert(
            state_addr2,
            Account {
                info: revm_state::AccountInfo {
                    balance: U256::from(200),
                    nonce: 2,
                    code_hash: Default::default(),
                    code: Default::default(),
                },
                transaction_id: Default::default(),
                storage: Default::default(),
                status: revm_state::AccountStatus::Touched,
            },
        );

        let source = StateChangeSource::Transaction(42);

        // Queue: [PrefetchProofs1, PrefetchProofs2, StateUpdate1, StateUpdate2, PrefetchProofs3]
        let tx = task.tx.clone();
        tx.send(MultiProofMessage::PrefetchProofs(targets1)).unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(targets2)).unwrap();
        tx.send(MultiProofMessage::StateUpdate(source.into(), state_update1)).unwrap();
        tx.send(MultiProofMessage::StateUpdate(source.into(), state_update2)).unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(targets3.clone())).unwrap();

        // Step 1: Receive and batch PrefetchProofs (should get targets1 + targets2)
        let mut pending_msg: Option<MultiProofMessage> = None;
        if let Ok(MultiProofMessage::PrefetchProofs(targets)) = task.rx.recv() {
            let mut merged_targets = targets;
            let mut num_batched = 1;

            loop {
                match task.rx.try_recv() {
                    Ok(MultiProofMessage::PrefetchProofs(next_targets)) => {
                        merged_targets.extend(next_targets);
                        num_batched += 1;
                    }
                    Ok(other_msg) => {
                        // Store locally to preserve ordering (the fix)
                        pending_msg = Some(other_msg);
                        break;
                    }
                    Err(_) => break,
                }
            }

            // Should have batched exactly 2 PrefetchProofs (not 3!)
            assert_eq!(num_batched, 2, "Should batch only until different message type");
            assert_eq!(merged_targets.len(), 2);
            assert!(merged_targets.contains_key(&addr1));
            assert!(merged_targets.contains_key(&addr2));
            assert!(!merged_targets.contains_key(&addr3), "addr3 should NOT be in first batch");
        } else {
            panic!("Expected PrefetchProofs message");
        }

        // Step 2: The pending message should be StateUpdate1 (preserved ordering)
        match pending_msg {
            Some(MultiProofMessage::StateUpdate(_src, update)) => {
                assert!(update.contains_key(&state_addr1), "Should be first StateUpdate");
            }
            _ => panic!("StateUpdate1 was lost or reordered! The ordering fix is broken."),
        }

        // Step 3: Next in channel should be StateUpdate2
        match task.rx.try_recv() {
            Ok(MultiProofMessage::StateUpdate(_src, update)) => {
                assert!(update.contains_key(&state_addr2), "Should be second StateUpdate");
            }
            _ => panic!("StateUpdate2 was lost!"),
        }

        // Step 4: Next in channel should be PrefetchProofs3
        match task.rx.try_recv() {
            Ok(MultiProofMessage::PrefetchProofs(targets)) => {
                assert_eq!(targets.len(), 1);
                assert!(targets.contains_key(&addr3));
            }
            _ => panic!("PrefetchProofs3 was lost!"),
        }
    }

    /// Verifies that a pending message is processed before the next loop iteration (ordering).
    #[test]
    fn test_pending_message_processed_before_next_iteration() {
        use alloy_evm::block::StateChangeSource;
        use revm_state::Account;

        let test_provider_factory = create_test_provider_factory();
        let test_provider = create_cached_provider(test_provider_factory.clone());
        let mut task = create_test_state_root_task(test_provider_factory);

        // Queue: Prefetch1, StateUpdate, Prefetch2
        let prefetch_addr1 = B256::random();
        let prefetch_addr2 = B256::random();
        let mut prefetch1 = MultiProofTargets::default();
        prefetch1.insert(prefetch_addr1, HashSet::default());
        let mut prefetch2 = MultiProofTargets::default();
        prefetch2.insert(prefetch_addr2, HashSet::default());

        let state_addr = alloy_primitives::Address::random();
        let mut state_update = EvmState::default();
        state_update.insert(
            state_addr,
            Account {
                info: revm_state::AccountInfo {
                    balance: U256::from(42),
                    nonce: 1,
                    code_hash: Default::default(),
                    code: Default::default(),
                },
                transaction_id: Default::default(),
                storage: Default::default(),
                status: revm_state::AccountStatus::Touched,
            },
        );

        let source = StateChangeSource::Transaction(99);

        let tx = task.tx.clone();
        tx.send(MultiProofMessage::PrefetchProofs(prefetch1)).unwrap();
        tx.send(MultiProofMessage::StateUpdate(source.into(), state_update)).unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(prefetch2.clone())).unwrap();

        let mut ctx = MultiproofBatchCtx::new(Instant::now());
        let mut batch_metrics = MultiproofBatchMetrics::default();

        // First message: Prefetch1 batches; StateUpdate becomes pending.
        let first = task.rx.recv().unwrap();
        assert!(matches!(first, MultiProofMessage::PrefetchProofs(_)));
        assert!(!task.process_multiproof_message(
            first,
            &mut ctx,
            &mut batch_metrics,
            &test_provider
        ));
        let pending = ctx.pending_msg.take().expect("pending message captured");
        assert!(matches!(pending, MultiProofMessage::StateUpdate(_, _)));

        // Pending message should be handled before the next select loop.
        // StateUpdate is processed directly without batching.
        assert!(!task.process_multiproof_message(
            pending,
            &mut ctx,
            &mut batch_metrics,
            &test_provider
        ));

        // Since StateUpdate doesn't batch, Prefetch2 remains in the channel (not in pending_msg).
        assert!(ctx.pending_msg.is_none());

        // Prefetch2 should still be in the channel.
        match task.rx.try_recv() {
            Ok(MultiProofMessage::PrefetchProofs(targets)) => {
                assert_eq!(targets.len(), 1);
                assert!(targets.contains_key(&prefetch_addr2));
            }
            other => panic!("Expected PrefetchProofs2 in channel, got {:?}", other),
        }
    }

    /// Verifies that BAL messages are processed correctly and generate state updates.
    #[test]
    fn test_bal_message_processing() {
        let test_provider_factory = create_test_provider_factory();
        let test_provider = create_cached_provider(test_provider_factory.clone());
        let mut task = create_test_state_root_task(test_provider_factory);

        // Create a simple BAL with one account change
        let account_address = Address::random();
        let account_changes = AccountChanges {
            address: account_address,
            balance_changes: vec![BalanceChange::new(0, U256::from(1000))],
            nonce_changes: vec![],
            code_changes: vec![],
            storage_changes: vec![],
            storage_reads: vec![],
        };

        let bal = Arc::new(vec![account_changes]);

        let mut ctx = MultiproofBatchCtx::new(Instant::now());
        let mut batch_metrics = MultiproofBatchMetrics::default();

        let should_finish = task.process_multiproof_message(
            MultiProofMessage::BlockAccessList(bal),
            &mut ctx,
            &mut batch_metrics,
            &test_provider,
        );

        // BAL should mark updates as finished
        assert!(ctx.updates_finished_time.is_some());

        // Should have dispatched state update proofs
        assert!(batch_metrics.state_update_proofs_requested > 0);

        // Should need to wait for the results of those proofs to arrive
        assert!(!should_finish, "Should continue waiting for proofs");
    }
}
