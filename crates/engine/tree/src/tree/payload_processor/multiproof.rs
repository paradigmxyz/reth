//! Multiproof task related functionality.

use alloy_evm::block::StateChangeSource;
use alloy_primitives::{
    keccak256,
    map::{B256Set, HashSet},
    B256,
};
use crossbeam_channel::{unbounded, Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use dashmap::DashMap;
use derive_more::derive::Deref;
use metrics::{Gauge, Histogram};
use reth_metrics::Metrics;
use reth_revm::state::EvmState;
use reth_trie::{
    added_removed_keys::MultiAddedRemovedKeys, DecodedMultiProof, HashedPostState, HashedStorage,
    MultiProofTargets,
};
use reth_trie_parallel::{
    proof::ParallelProof,
    proof_task::{
        AccountMultiproofInput, ProofResultContext, ProofResultMessage, ProofWorkerHandle,
        StorageProofInput,
    },
};
use std::{collections::BTreeMap, mem, ops::DerefMut, sync::Arc, time::Instant};
use tracing::{debug, error, instrument, trace};

/// Maximum number of targets to batch together for prefetch batching.
/// Prefetches are just proof requests (no state merging), so we allow a higher cap than state
/// updates
const PREFETCH_MAX_BATCH_TARGETS: usize = 512;

/// Maximum number of prefetch messages to batch together.
/// Prevents excessive batching even with small messages.
const PREFETCH_MAX_BATCH_MESSAGES: usize = 16;

/// Maximum number of targets to batch together for state updates.
/// Lower than prefetch because state updates require additional processing (hashing, state
/// partitioning) before dispatch.
const STATE_UPDATE_MAX_BATCH_TARGETS: usize = 64;

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
    StateUpdate(StateChangeSource, EvmState),
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

        // return early if we don't have the next expected proof
        if !self.pending_proofs.contains_key(&self.next_to_deliver) {
            return Vec::new()
        }

        let mut consecutive_proofs = Vec::with_capacity(self.pending_proofs.len());
        let mut current_sequence = self.next_to_deliver;

        // keep collecting proofs and state updates as long as we have consecutive sequence numbers
        while let Some(pending) = self.pending_proofs.remove(&current_sequence) {
            consecutive_proofs.push(pending);
            current_sequence += 1;

            // if we don't have the next number, stop collecting
            if !self.pending_proofs.contains_key(&current_sequence) {
                break;
            }
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

/// A pending multiproof task, either [`StorageMultiproofInput`] or [`MultiproofInput`].
#[derive(Debug)]
enum PendingMultiproofTask {
    /// A storage multiproof task input.
    Storage(StorageMultiproofInput),
    /// A regular multiproof task input.
    Regular(MultiproofInput),
}

impl PendingMultiproofTask {
    /// Returns the proof sequence number of the task.
    const fn proof_sequence_number(&self) -> u64 {
        match self {
            Self::Storage(input) => input.proof_sequence_number,
            Self::Regular(input) => input.proof_sequence_number,
        }
    }

    /// Returns whether or not the proof targets are empty.
    fn proof_targets_is_empty(&self) -> bool {
        match self {
            Self::Storage(input) => input.proof_targets.is_empty(),
            Self::Regular(input) => input.proof_targets.is_empty(),
        }
    }

    /// Destroys the input and sends a [`MultiProofMessage::EmptyProof`] message to the sender.
    fn send_empty_proof(self) {
        match self {
            Self::Storage(input) => input.send_empty_proof(),
            Self::Regular(input) => input.send_empty_proof(),
        }
    }
}

impl From<StorageMultiproofInput> for PendingMultiproofTask {
    fn from(input: StorageMultiproofInput) -> Self {
        Self::Storage(input)
    }
}

impl From<MultiproofInput> for PendingMultiproofTask {
    fn from(input: MultiproofInput) -> Self {
        Self::Regular(input)
    }
}

/// Input parameters for dispatching a dedicated storage multiproof calculation.
#[derive(Debug)]
struct StorageMultiproofInput {
    hashed_state_update: HashedPostState,
    hashed_address: B256,
    proof_targets: B256Set,
    proof_sequence_number: u64,
    state_root_message_sender: CrossbeamSender<MultiProofMessage>,
    multi_added_removed_keys: Arc<MultiAddedRemovedKeys>,
}

impl StorageMultiproofInput {
    /// Destroys the input and sends a [`MultiProofMessage::EmptyProof`] message to the sender.
    fn send_empty_proof(self) {
        let _ = self.state_root_message_sender.send(MultiProofMessage::EmptyProof {
            sequence_number: self.proof_sequence_number,
            state: self.hashed_state_update,
        });
    }
}

/// Input parameters for dispatching a multiproof calculation.
#[derive(Debug)]
struct MultiproofInput {
    source: Option<StateChangeSource>,
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
    /// Cached storage proof roots for missed leaves; this maps
    /// hashed (missed) addresses to their storage proof roots.
    ///
    /// It is important to cache these. Otherwise, a common account
    /// (popular ERC-20, etc.) having missed leaves in its path would
    /// repeatedly calculate these proofs per interacting transaction
    /// (same account different slots).
    ///
    /// This also works well with chunking multiproofs, which may break
    /// a big account change into different chunks, which may repeatedly
    /// revisit missed leaves.
    missed_leaves_storage_roots: Arc<DashMap<B256, B256>>,
    /// Channel sender cloned into each dispatched job so workers can send back the
    /// `ProofResultMessage`.
    proof_result_tx: CrossbeamSender<ProofResultMessage>,
    /// Metrics
    metrics: MultiProofTaskMetrics,
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

        Self {
            metrics,
            proof_worker_handle,
            missed_leaves_storage_roots: Default::default(),
            proof_result_tx,
        }
    }

    /// Dispatches a new multiproof calculation to worker pools.
    fn dispatch(&self, input: PendingMultiproofTask) {
        // If there are no proof targets, we can just send an empty multiproof back immediately
        if input.proof_targets_is_empty() {
            debug!(
                sequence_number = input.proof_sequence_number(),
                "No proof targets, sending empty multiproof back immediately"
            );
            input.send_empty_proof();
            return;
        }

        match input {
            PendingMultiproofTask::Storage(storage_input) => {
                self.dispatch_storage_proof(storage_input);
            }
            PendingMultiproofTask::Regular(multiproof_input) => {
                self.dispatch_multiproof(multiproof_input);
            }
        }
    }

    /// Dispatches a single storage proof calculation to worker pool.
    fn dispatch_storage_proof(&self, storage_multiproof_input: StorageMultiproofInput) {
        let StorageMultiproofInput {
            hashed_state_update,
            hashed_address,
            proof_targets,
            proof_sequence_number,
            multi_added_removed_keys,
            state_root_message_sender: _,
        } = storage_multiproof_input;

        let storage_targets = proof_targets.len();

        trace!(
            target: "engine::tree::payload_processor::multiproof",
            proof_sequence_number,
            ?proof_targets,
            storage_targets,
            "Dispatching storage proof to workers"
        );

        let start = Instant::now();

        // Create prefix set from targets
        let prefix_set = reth_trie::prefix_set::PrefixSetMut::from(
            proof_targets.iter().map(reth_trie::Nibbles::unpack),
        );
        let prefix_set = prefix_set.freeze();

        // Build computation input (data only)
        let input = StorageProofInput::new(
            hashed_address,
            prefix_set,
            proof_targets,
            true, // with_branch_node_masks
            Some(multi_added_removed_keys),
        );

        // Dispatch to storage worker
        if let Err(e) = self.proof_worker_handle.dispatch_storage_proof(
            input,
            ProofResultContext::new(
                self.proof_result_tx.clone(),
                proof_sequence_number,
                hashed_state_update,
                start,
            ),
        ) {
            error!(target: "engine::tree::payload_processor::multiproof", ?e, "Failed to dispatch storage proof");
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

        let missed_leaves_storage_roots = self.missed_leaves_storage_roots.clone();
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
            missed_leaves_storage_roots,
            // Workers will send ProofResultMessage directly to proof_result_rx
            proof_result_sender: ProofResultContext::new(
                self.proof_result_tx.clone(),
                proof_sequence_number,
                hashed_state_update,
                start,
            ),
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
    /// Histogram of state update batch sizes (number of messages merged).
    pub state_update_batch_size_histogram: Histogram,

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

/// Context for multiproof message batching loop.
///
/// Contains processing state that persists across loop iterations.
struct MultiproofBatchCtx {
    /// Message encountered during batching that couldn't be merged (different type).
    /// Processed first in next iteration to preserve message ordering.
    pending_msg: Option<MultiProofMessage>,
    /// Timestamp when the first state update or prefetch was received.
    first_update_time: Option<Instant>,
    /// Timestamp before the first state update or prefetch was received.
    start: Instant,
    /// Whether all state updates have been received.
    updates_finished: bool,
    /// Timestamp when state updates finished.
    updates_finished_time: Option<Instant>,
}

impl MultiproofBatchCtx {
    /// Creates a new batch context with the given start time.
    const fn new(start: Instant) -> Self {
        Self {
            pending_msg: None,
            first_update_time: None,
            start,
            updates_finished: false,
            updates_finished_time: None,
        }
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

impl MultiProofTask {
    /// Creates a multiproof task with separate channels: control on `tx`/`rx`, proof results on
    /// `proof_result_rx`.
    pub(super) fn new(
        proof_worker_handle: ProofWorkerHandle,
        to_sparse_trie: std::sync::mpsc::Sender<SparseTrieUpdate>,
        chunk_size: Option<usize>,
    ) -> Self {
        let (tx, rx) = unbounded();
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

    /// Returns a sender that can be used to send arbitrary [`MultiProofMessage`]s to this task.
    pub(super) fn state_root_message_sender(&self) -> CrossbeamSender<MultiProofMessage> {
        self.tx.clone()
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
    fn on_prefetch_proof(&mut self, targets: MultiProofTargets) -> u64 {
        let proof_targets = self.get_prefetch_proof_targets(targets);
        self.fetched_proof_targets.extend_ref(&proof_targets);

        // Make sure all target accounts have an `AddedRemovedKeySet` in the
        // [`MultiAddedRemovedKeys`]. Even if there are not any known removed keys for the account,
        // we still want to optimistically fetch extension children for the leaf addition case.
        self.multi_added_removed_keys.touch_accounts(proof_targets.keys().copied());

        // Clone+Arc MultiAddedRemovedKeys for sharing with the dispatched multiproof tasks
        let multi_added_removed_keys = Arc::new(self.multi_added_removed_keys.clone());

        self.metrics.prefetch_proof_targets_accounts_histogram.record(proof_targets.len() as f64);
        self.metrics
            .prefetch_proof_targets_storages_histogram
            .record(proof_targets.values().map(|slots| slots.len()).sum::<usize>() as f64);

        let chunking_len = proof_targets.chunking_length();
        let available_account_workers =
            self.multiproof_manager.proof_worker_handle.available_account_workers();
        let available_storage_workers =
            self.multiproof_manager.proof_worker_handle.available_storage_workers();
        let num_chunks = dispatch_with_chunking(
            proof_targets,
            chunking_len,
            self.chunk_size,
            self.max_targets_for_chunking,
            available_account_workers,
            available_storage_workers,
            MultiProofTargets::chunks,
            |proof_targets| {
                self.multiproof_manager.dispatch(
                    MultiproofInput {
                        source: None,
                        hashed_state_update: Default::default(),
                        proof_targets,
                        proof_sequence_number: self.proof_sequencer.next_sequence(),
                        state_root_message_sender: self.tx.clone(),
                        multi_added_removed_keys: Some(multi_added_removed_keys.clone()),
                    }
                    .into(),
                );
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

    /// Calls `get_proof_targets` with existing proof targets for prefetching.
    fn get_prefetch_proof_targets(&self, mut targets: MultiProofTargets) -> MultiProofTargets {
        // Here we want to filter out any targets that are already fetched
        //
        // This means we need to remove any storage slots that have already been fetched
        let mut duplicates = 0;

        // First remove all storage targets that are subsets of already fetched storage slots
        targets.retain(|hashed_address, target_storage| {
            let keep = self
                .fetched_proof_targets
                .get(hashed_address)
                // do NOT remove if None, because that means the account has not been fetched yet
                .is_none_or(|fetched_storage| {
                    // remove if a subset
                    !target_storage.is_subset(fetched_storage)
                });

            if !keep {
                duplicates += target_storage.len();
            }

            keep
        });

        // For all non-subset remaining targets, we have to calculate the difference
        for (hashed_address, target_storage) in targets.deref_mut() {
            let Some(fetched_storage) = self.fetched_proof_targets.get(hashed_address) else {
                // this means the account has not been fetched yet, so we must fetch everything
                // associated with this account
                continue;
            };

            let prev_target_storage_len = target_storage.len();

            // keep only the storage slots that have not been fetched yet
            //
            // we already removed subsets, so this should only remove duplicates
            target_storage.retain(|slot| !fetched_storage.contains(slot));

            duplicates += prev_target_storage_len - target_storage.len();
        }

        if duplicates > 0 {
            trace!(target: "engine::tree::payload_processor::multiproof", duplicates, "Removed duplicate prefetch proof targets");
        }

        targets
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
    fn on_state_update(&mut self, source: StateChangeSource, update: EvmState) -> u64 {
        let hashed_state_update = evm_state_to_hashed_post_state(update);

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

                self.multiproof_manager.dispatch(
                    MultiproofInput {
                        source: Some(source),
                        hashed_state_update,
                        proof_targets,
                        proof_sequence_number: self.proof_sequencer.next_sequence(),
                        state_root_message_sender: self.tx.clone(),
                        multi_added_removed_keys: Some(multi_added_removed_keys.clone()),
                    }
                    .into(),
                );
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

    /// Processes a multiproof message, batching consecutive same-type messages.
    ///
    /// Drains queued messages of the same type and merges them into one batch before processing,
    /// storing one pending message (different type or over-cap) to handle on the next iteration.
    /// This preserves ordering without requeuing onto the channel.
    ///
    /// Returns `true` if done, `false` to continue.
    fn process_multiproof_message(
        &mut self,
        msg: MultiProofMessage,
        ctx: &mut MultiproofBatchCtx,
        batch_metrics: &mut MultiproofBatchMetrics,
    ) -> bool {
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

                // Accumulate messages including the first one
                let mut accumulated_count = targets.chunking_length();
                // Preallocate up to the message cap; avoids repeated reallocations when bursts
                // arrive.
                let mut accumulated_targets: Vec<MultiProofTargets> =
                    Vec::with_capacity(PREFETCH_MAX_BATCH_MESSAGES);
                accumulated_targets.push(targets);

                // Fast-path dispatch if the first message already fills the batch.
                if accumulated_count < PREFETCH_MAX_BATCH_TARGETS
                    && accumulated_targets.len() < PREFETCH_MAX_BATCH_MESSAGES
                {
                    loop {
                        if accumulated_count >= PREFETCH_MAX_BATCH_TARGETS
                            || accumulated_targets.len() >= PREFETCH_MAX_BATCH_MESSAGES
                        {
                            break;
                        }
                        match self.rx.try_recv() {
                            Ok(MultiProofMessage::PrefetchProofs(next_targets)) => {
                                let next_count = next_targets.chunking_length();
                                if accumulated_count + next_count > PREFETCH_MAX_BATCH_TARGETS {
                                    ctx.pending_msg =
                                        Some(MultiProofMessage::PrefetchProofs(next_targets));
                                    break;
                                }
                                accumulated_count += next_count;
                                accumulated_targets.push(next_targets);
                            }
                            Ok(other_msg) => {
                                ctx.pending_msg = Some(other_msg);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                }

                // Process all accumulated messages in a single batch
                let num_batched = accumulated_targets.len();
                self.metrics.prefetch_batch_size_histogram.record(num_batched as f64);

                // Merge all accumulated prefetch targets into a single dispatch payload.
                let mut accumulated_iter = accumulated_targets.into_iter();
                let mut merged_targets =
                    accumulated_iter.next().expect("prefetch batch always has at least one entry");
                for next_targets in accumulated_iter {
                    merged_targets.extend(next_targets);
                }

                let account_targets = merged_targets.len();
                let storage_targets =
                    merged_targets.values().map(|slots| slots.len()).sum::<usize>();
                batch_metrics.prefetch_proofs_requested += self.on_prefetch_proof(merged_targets);
                debug!(
                    target: "engine::tree::payload_processor::multiproof",
                    account_targets,
                    storage_targets,
                    prefetch_proofs_requested = batch_metrics.prefetch_proofs_requested,
                    num_batched,
                    "Dispatched prefetch batch"
                );

                false
            }
            // State update: batch consecutive updates from the same source
            MultiProofMessage::StateUpdate(source, update) => {
                trace!(target: "engine::tree::payload_processor::multiproof", "processing MultiProofMessage::StateUpdate");

                if ctx.first_update_time.is_none() {
                    self.metrics
                        .first_update_wait_time_histogram
                        .record(ctx.start.elapsed().as_secs_f64());
                    ctx.first_update_time = Some(Instant::now());
                    debug!(target: "engine::tree::payload_processor::multiproof", "Started state root calculation");
                }

                // Accumulate messages including the first one
                let mut accumulated_targets = estimate_evm_state_targets(&update);
                // Preallocate modestly; state updates are heavier per message, but we can see small
                // bursts.
                let mut accumulated_updates: Vec<(StateChangeSource, EvmState)> =
                    Vec::with_capacity(16);
                accumulated_updates.push((source, update));

                loop {
                    if accumulated_targets >= STATE_UPDATE_MAX_BATCH_TARGETS {
                        break;
                    }
                    match self.rx.try_recv() {
                        Ok(MultiProofMessage::StateUpdate(next_source, next_update)) => {
                            let (batch_source, batch_update) = &accumulated_updates[0];
                            if !can_batch_state_update(
                                *batch_source,
                                batch_update,
                                next_source,
                                &next_update,
                            ) {
                                ctx.pending_msg =
                                    Some(MultiProofMessage::StateUpdate(next_source, next_update));
                                break;
                            }

                            let next_estimate = estimate_evm_state_targets(&next_update);
                            // Do not merge outlier updates that exceed the cap on their own.
                            // Leave them pending so they dispatch immediately on the next loop.
                            if next_estimate > STATE_UPDATE_MAX_BATCH_TARGETS {
                                ctx.pending_msg =
                                    Some(MultiProofMessage::StateUpdate(next_source, next_update));
                                break;
                            }
                            if accumulated_targets + next_estimate > STATE_UPDATE_MAX_BATCH_TARGETS
                            {
                                ctx.pending_msg =
                                    Some(MultiProofMessage::StateUpdate(next_source, next_update));
                                break;
                            }
                            accumulated_targets += next_estimate;
                            accumulated_updates.push((next_source, next_update));
                        }
                        Ok(other_msg) => {
                            ctx.pending_msg = Some(other_msg);
                            break;
                        }
                        Err(_) => break,
                    }
                }

                // Process all accumulated messages in a single batch
                let num_batched = accumulated_updates.len();
                self.metrics.state_update_batch_size_histogram.record(num_batched as f64);

                let batch_source = accumulated_updates[0].0;
                {
                    let batch_update = &accumulated_updates[0].1;
                    debug_assert!(accumulated_updates.iter().all(|(source, update)| {
                        can_batch_state_update(batch_source, batch_update, *source, update)
                    }));
                }

                // Merge all accumulated updates into a single EvmState payload.
                let mut accumulated_iter = accumulated_updates.into_iter();
                let (batch_source, mut merged_update) = accumulated_iter
                    .next()
                    .expect("state update batch always has at least one entry");
                for (_, next_update) in accumulated_iter {
                    merged_update.extend(next_update);
                }

                let batch_len = merged_update.len();
                batch_metrics.state_update_proofs_requested +=
                    self.on_state_update(batch_source, merged_update);
                debug!(
                    target: "engine::tree::payload_processor::multiproof",
                    ?batch_source,
                    len = batch_len,
                    state_update_proofs_requested = ?batch_metrics.state_update_proofs_requested,
                    num_batched,
                    "Dispatched state update batch"
                );

                false
            }
            // Signal that no more state updates will arrive
            MultiProofMessage::FinishedStateUpdates => {
                trace!(target: "engine::tree::payload_processor::multiproof", "processing MultiProofMessage::FinishedStateUpdates");

                ctx.updates_finished = true;
                ctx.updates_finished_time = Some(Instant::now());

                if self.is_done(
                    batch_metrics.proofs_processed,
                    batch_metrics.state_update_proofs_requested,
                    batch_metrics.prefetch_proofs_requested,
                    ctx.updates_finished,
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
                    ctx.updates_finished,
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
    /// 6. While running, consecutive [`MultiProofMessage::PrefetchProofs`] and
    ///    [`MultiProofMessage::StateUpdate`] messages are batched to reduce redundant work; if a
    ///    different message type arrives mid-batch or a batch cap is reached, it is held as
    ///    `pending_msg` and processed on the next loop to preserve ordering.
    /// 7. This task exits after all pending proofs are processed.
    #[instrument(
        level = "debug",
        name = "MultiProofTask::run",
        target = "engine::tree::payload_processor::multiproof",
        skip_all
    )]
    pub(crate) fn run(mut self) {
        let mut ctx = MultiproofBatchCtx::new(Instant::now());
        let mut batch_metrics = MultiproofBatchMetrics::default();

        'main: loop {
            trace!(target: "engine::tree::payload_processor::multiproof", "entering main channel receiving loop");

            // Always drain proof results first to maintain priority over pending messages.
            // This prevents priority inversion where pending_msg would bypass select_biased!
            // and starve proof result processing.
            while let Ok(proof_result) = self.proof_result_rx.try_recv() {
                batch_metrics.proofs_processed += 1;

                self.metrics.proof_calculation_duration_histogram.record(proof_result.elapsed);

                self.multiproof_manager.on_calculation_complete();

                match proof_result.result {
                    Ok(proof_result_data) => {
                        debug!(
                            target: "engine::tree::payload_processor::multiproof",
                            sequence = proof_result.sequence_number,
                            total_proofs = batch_metrics.proofs_processed,
                            "Processing calculated proof from worker (priority drain)"
                        );

                        let update = SparseTrieUpdate {
                            state: proof_result.state,
                            multiproof: proof_result_data.into_multiproof(),
                        };

                        if let Some(combined_update) =
                            self.on_proof(proof_result.sequence_number, update)
                        {
                            let _ = self.to_sparse_trie.send(combined_update);
                        }
                    }
                    Err(error) => {
                        error!(target: "engine::tree::payload_processor::multiproof", ?error, "proof calculation error from worker");
                        return;
                    }
                }

                if self.is_done(
                    batch_metrics.proofs_processed,
                    batch_metrics.state_update_proofs_requested,
                    batch_metrics.prefetch_proofs_requested,
                    ctx.updates_finished,
                ) {
                    debug!(
                        target: "engine::tree::payload_processor::multiproof",
                        "State updates finished and all proofs processed, ending calculation"
                    );
                    break 'main;
                }
            }

            if let Some(msg) = ctx.pending_msg.take() {
                if self.process_multiproof_message(msg, &mut ctx, &mut batch_metrics) {
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
                                    debug!(
                                        target: "engine::tree::payload_processor::multiproof",
                                        sequence = proof_result.sequence_number,
                                        total_proofs = batch_metrics.proofs_processed,
                                        "Processing calculated proof from worker"
                                    );

                                    let update = SparseTrieUpdate {
                                        state: proof_result.state,
                                        multiproof: proof_result_data.into_multiproof(),
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
                                ctx.updates_finished,
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

                    if self.process_multiproof_message(msg, &mut ctx, &mut batch_metrics) {
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
    for &hashed_address in state_update.accounts.keys() {
        if !fetched_proof_targets.contains_key(&hashed_address) {
            targets.insert(hashed_address, HashSet::default());
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
                !fetched.is_some_and(|f| f.contains(*slot))
                    || storage_added_removed_keys.is_some_and(|k| k.is_removed(slot))
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
    let should_chunk = chunking_len > max_targets_for_chunking
        || available_account_workers > 1
        || available_storage_workers > 1;

    if should_chunk
        && let Some(chunk_size) = chunk_size
        && chunking_len > chunk_size
    {
        let mut dispatched = 0usize;
        for chunk in chunker(items, chunk_size) {
            dispatch(chunk);
            dispatched += 1;
        }
        return dispatched;
    }

    dispatch(items);
    1
}

/// Checks whether two state updates can be merged in a batch.
///
/// Transaction updates that share the same `StateChangeSource::Transaction` are safe to merge
/// because they originate from one logical execution and can be coalesced to amortize proof work.
fn can_batch_state_update(
    batch_source: StateChangeSource,
    batch_update: &EvmState,
    next_source: StateChangeSource,
    next_update: &EvmState,
) -> bool {
    if !same_state_change_source(batch_source, next_source) {
        return false;
    }

    match (batch_source, next_source) {
        (StateChangeSource::PreBlock(_), StateChangeSource::PreBlock(_))
        | (StateChangeSource::PostBlock(_), StateChangeSource::PostBlock(_)) => {
            batch_update == next_update
        }
        _ => true,
    }
}

/// Checks whether two state change sources refer to the same origin.
fn same_state_change_source(lhs: StateChangeSource, rhs: StateChangeSource) -> bool {
    match (lhs, rhs) {
        (StateChangeSource::Transaction(a), StateChangeSource::Transaction(b)) => a == b,
        (StateChangeSource::PreBlock(a), StateChangeSource::PreBlock(b)) => {
            mem::discriminant(&a) == mem::discriminant(&b)
        }
        (StateChangeSource::PostBlock(a), StateChangeSource::PostBlock(b)) => {
            mem::discriminant(&a) == mem::discriminant(&b)
        }
        _ => false,
    }
}

/// Estimates target count from `EvmState` for batching decisions.
fn estimate_evm_state_targets(state: &EvmState) -> usize {
    state
        .values()
        .filter(|account| account.is_touched())
        .map(|account| {
            let changed_slots = account.storage.iter().filter(|(_, v)| v.is_changed()).count();
            1 + changed_slots
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::map::B256Set;
    use reth_provider::{
        providers::OverlayStateProviderFactory, test_utils::create_test_provider_factory,
        BlockReader, DatabaseProviderFactory, PruneCheckpointReader, StageCheckpointReader,
        TrieReader,
    };
    use reth_trie::MultiProof;
    use reth_trie_parallel::proof_task::{ProofTaskCtx, ProofWorkerHandle};
    use revm_primitives::{B256, U256};
    use std::sync::OnceLock;
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
                Provider: BlockReader + TrieReader + StageCheckpointReader + PruneCheckpointReader,
            > + Clone
            + Send
            + 'static,
    {
        let rt_handle = get_test_runtime_handle();
        let overlay_factory = OverlayStateProviderFactory::new(factory);
        let task_ctx = ProofTaskCtx::new(overlay_factory);
        let proof_handle = ProofWorkerHandle::new(rt_handle, task_ctx, 1, 1);
        let (to_sparse_trie, _receiver) = std::sync::mpsc::channel();

        MultiProofTask::new(proof_handle, to_sparse_trie, Some(1))
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
    fn test_get_prefetch_proof_targets_no_duplicates() {
        let test_provider_factory = create_test_provider_factory();
        let mut test_state_root_task = create_test_state_root_task(test_provider_factory);

        // populate some targets
        let mut targets = MultiProofTargets::default();
        let addr1 = B256::random();
        let addr2 = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();
        targets.insert(addr1, std::iter::once(slot1).collect());
        targets.insert(addr2, std::iter::once(slot2).collect());

        let prefetch_proof_targets =
            test_state_root_task.get_prefetch_proof_targets(targets.clone());

        // check that the prefetch proof targets are the same because there are no fetched proof
        // targets yet
        assert_eq!(prefetch_proof_targets, targets);

        // add a different addr and slot to fetched proof targets
        let addr3 = B256::random();
        let slot3 = B256::random();
        test_state_root_task.fetched_proof_targets.insert(addr3, std::iter::once(slot3).collect());

        let prefetch_proof_targets =
            test_state_root_task.get_prefetch_proof_targets(targets.clone());

        // check that the prefetch proof targets are the same because the fetched proof targets
        // don't overlap with the prefetch targets
        assert_eq!(prefetch_proof_targets, targets);
    }

    #[test]
    fn test_get_prefetch_proof_targets_remove_subset() {
        let test_provider_factory = create_test_provider_factory();
        let mut test_state_root_task = create_test_state_root_task(test_provider_factory);

        // populate some targe
        let mut targets = MultiProofTargets::default();
        let addr1 = B256::random();
        let addr2 = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();
        targets.insert(addr1, std::iter::once(slot1).collect());
        targets.insert(addr2, std::iter::once(slot2).collect());

        // add a subset of the first target to fetched proof targets
        test_state_root_task.fetched_proof_targets.insert(addr1, std::iter::once(slot1).collect());

        let prefetch_proof_targets =
            test_state_root_task.get_prefetch_proof_targets(targets.clone());

        // check that the prefetch proof targets do not include the subset
        assert_eq!(prefetch_proof_targets.len(), 1);
        assert!(!prefetch_proof_targets.contains_key(&addr1));
        assert!(prefetch_proof_targets.contains_key(&addr2));

        // now add one more slot to the prefetch targets
        let slot3 = B256::random();
        targets.get_mut(&addr1).unwrap().insert(slot3);

        let prefetch_proof_targets =
            test_state_root_task.get_prefetch_proof_targets(targets.clone());

        // check that the prefetch proof targets do not include the subset
        // but include the new slot
        assert_eq!(prefetch_proof_targets.len(), 2);
        assert!(prefetch_proof_targets.contains_key(&addr1));
        assert_eq!(
            *prefetch_proof_targets.get(&addr1).unwrap(),
            std::iter::once(slot3).collect::<B256Set>()
        );
        assert!(prefetch_proof_targets.contains_key(&addr2));
        assert_eq!(
            *prefetch_proof_targets.get(&addr2).unwrap(),
            std::iter::once(slot2).collect::<B256Set>()
        );
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

        let tx = task.state_root_message_sender();
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

    /// Verifies that consecutive state update messages from the same source are batched together.
    #[test]
    fn test_state_update_batching() {
        use alloy_evm::block::StateChangeSource;
        use revm_state::Account;

        let test_provider_factory = create_test_provider_factory();
        let mut task = create_test_state_root_task(test_provider_factory);

        // create multiple state updates
        let addr1 = alloy_primitives::Address::random();
        let addr2 = alloy_primitives::Address::random();

        let mut update1 = EvmState::default();
        update1.insert(
            addr1,
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

        let mut update2 = EvmState::default();
        update2.insert(
            addr2,
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

        let source = StateChangeSource::Transaction(0);

        let tx = task.state_root_message_sender();
        tx.send(MultiProofMessage::StateUpdate(source, update1.clone())).unwrap();
        tx.send(MultiProofMessage::StateUpdate(source, update2.clone())).unwrap();

        let proofs_requested =
            if let Ok(MultiProofMessage::StateUpdate(_src, update)) = task.rx.recv() {
                let mut merged_update = update;
                let mut num_batched = 1;

                while let Ok(MultiProofMessage::StateUpdate(_next_source, next_update)) =
                    task.rx.try_recv()
                {
                    merged_update.extend(next_update);
                    num_batched += 1;
                }

                assert_eq!(num_batched, 2);
                assert_eq!(merged_update.len(), 2);
                assert!(merged_update.contains_key(&addr1));
                assert!(merged_update.contains_key(&addr2));

                task.on_state_update(source, merged_update)
            } else {
                panic!("Expected StateUpdate message");
            };
        assert_eq!(proofs_requested, 1);
    }

    /// Verifies that state updates from different sources are not batched together.
    #[test]
    fn test_state_update_batching_separates_sources() {
        use alloy_evm::block::StateChangeSource;
        use revm_state::Account;

        let test_provider_factory = create_test_provider_factory();
        let task = create_test_state_root_task(test_provider_factory);

        let addr_a1 = alloy_primitives::Address::random();
        let addr_b1 = alloy_primitives::Address::random();
        let addr_a2 = alloy_primitives::Address::random();

        let create_state_update = |addr: alloy_primitives::Address, balance: u64| {
            let mut state = EvmState::default();
            state.insert(
                addr,
                Account {
                    info: revm_state::AccountInfo {
                        balance: U256::from(balance),
                        nonce: 1,
                        code_hash: Default::default(),
                        code: Default::default(),
                    },
                    transaction_id: Default::default(),
                    storage: Default::default(),
                    status: revm_state::AccountStatus::Touched,
                },
            );
            state
        };

        let source_a = StateChangeSource::Transaction(1);
        let source_b = StateChangeSource::Transaction(2);

        // Queue: A1 (immediate dispatch), B1 (batched), A2 (should become pending)
        let tx = task.state_root_message_sender();
        tx.send(MultiProofMessage::StateUpdate(source_a, create_state_update(addr_a1, 100)))
            .unwrap();
        tx.send(MultiProofMessage::StateUpdate(source_b, create_state_update(addr_b1, 200)))
            .unwrap();
        tx.send(MultiProofMessage::StateUpdate(source_a, create_state_update(addr_a2, 300)))
            .unwrap();

        let mut pending_msg: Option<MultiProofMessage> = None;

        if let Ok(MultiProofMessage::StateUpdate(first_source, _)) = task.rx.recv() {
            assert!(same_state_change_source(first_source, source_a));

            // Simulate batching loop for remaining messages
            let mut accumulated_updates: Vec<(StateChangeSource, EvmState)> = Vec::new();
            let mut accumulated_targets = 0usize;

            loop {
                if accumulated_targets >= STATE_UPDATE_MAX_BATCH_TARGETS {
                    break;
                }
                match task.rx.try_recv() {
                    Ok(MultiProofMessage::StateUpdate(next_source, next_update)) => {
                        if let Some((batch_source, batch_update)) = accumulated_updates.first()
                            && !can_batch_state_update(
                                *batch_source,
                                batch_update,
                                next_source,
                                &next_update,
                            )
                        {
                            pending_msg =
                                Some(MultiProofMessage::StateUpdate(next_source, next_update));
                            break;
                        }

                        let next_estimate = estimate_evm_state_targets(&next_update);
                        if next_estimate > STATE_UPDATE_MAX_BATCH_TARGETS {
                            pending_msg =
                                Some(MultiProofMessage::StateUpdate(next_source, next_update));
                            break;
                        }
                        if accumulated_targets + next_estimate > STATE_UPDATE_MAX_BATCH_TARGETS
                            && !accumulated_updates.is_empty()
                        {
                            pending_msg =
                                Some(MultiProofMessage::StateUpdate(next_source, next_update));
                            break;
                        }
                        accumulated_targets += next_estimate;
                        accumulated_updates.push((next_source, next_update));
                    }
                    Ok(other_msg) => {
                        pending_msg = Some(other_msg);
                        break;
                    }
                    Err(_) => break,
                }
            }

            assert_eq!(accumulated_updates.len(), 1, "Should only batch matching sources");
            let batch_source = accumulated_updates[0].0;
            assert!(same_state_change_source(batch_source, source_b));

            let batch_source = accumulated_updates[0].0;
            let mut merged_update = accumulated_updates.remove(0).1;
            for (_, next_update) in accumulated_updates {
                merged_update.extend(next_update);
            }

            assert!(
                same_state_change_source(batch_source, source_b),
                "Batch should use matching source"
            );
            assert!(merged_update.contains_key(&addr_b1));
            assert!(!merged_update.contains_key(&addr_a1));
            assert!(!merged_update.contains_key(&addr_a2));
        } else {
            panic!("Expected first StateUpdate");
        }

        match pending_msg {
            Some(MultiProofMessage::StateUpdate(pending_source, pending_update)) => {
                assert!(same_state_change_source(pending_source, source_a));
                assert!(pending_update.contains_key(&addr_a2));
            }
            other => panic!("Expected pending StateUpdate with source_a, got {:?}", other),
        }
    }

    /// Verifies that pre-block updates only batch when their payloads are identical.
    #[test]
    fn test_pre_block_updates_require_payload_match_to_batch() {
        use alloy_evm::block::{StateChangePreBlockSource, StateChangeSource};
        use revm_state::Account;

        let test_provider_factory = create_test_provider_factory();
        let task = create_test_state_root_task(test_provider_factory);

        let addr1 = alloy_primitives::Address::random();
        let addr2 = alloy_primitives::Address::random();
        let addr3 = alloy_primitives::Address::random();

        let create_state_update = |addr: alloy_primitives::Address, balance: u64| {
            let mut state = EvmState::default();
            state.insert(
                addr,
                Account {
                    info: revm_state::AccountInfo {
                        balance: U256::from(balance),
                        nonce: 1,
                        code_hash: Default::default(),
                        code: Default::default(),
                    },
                    transaction_id: Default::default(),
                    storage: Default::default(),
                    status: revm_state::AccountStatus::Touched,
                },
            );
            state
        };

        let source = StateChangeSource::PreBlock(StateChangePreBlockSource::BeaconRootContract);

        // Queue: first update dispatched immediately, next two should not merge
        let tx = task.state_root_message_sender();
        tx.send(MultiProofMessage::StateUpdate(source, create_state_update(addr1, 100))).unwrap();
        tx.send(MultiProofMessage::StateUpdate(source, create_state_update(addr2, 200))).unwrap();
        tx.send(MultiProofMessage::StateUpdate(source, create_state_update(addr3, 300))).unwrap();

        let mut pending_msg: Option<MultiProofMessage> = None;

        if let Ok(MultiProofMessage::StateUpdate(first_source, first_update)) = task.rx.recv() {
            assert!(same_state_change_source(first_source, source));
            assert!(first_update.contains_key(&addr1));

            let mut accumulated_updates: Vec<(StateChangeSource, EvmState)> = Vec::new();
            let mut accumulated_targets = 0usize;

            loop {
                if accumulated_targets >= STATE_UPDATE_MAX_BATCH_TARGETS {
                    break;
                }
                match task.rx.try_recv() {
                    Ok(MultiProofMessage::StateUpdate(next_source, next_update)) => {
                        if let Some((batch_source, batch_update)) = accumulated_updates.first()
                            && !can_batch_state_update(
                                *batch_source,
                                batch_update,
                                next_source,
                                &next_update,
                            )
                        {
                            pending_msg =
                                Some(MultiProofMessage::StateUpdate(next_source, next_update));
                            break;
                        }

                        let next_estimate = estimate_evm_state_targets(&next_update);
                        if next_estimate > STATE_UPDATE_MAX_BATCH_TARGETS {
                            pending_msg =
                                Some(MultiProofMessage::StateUpdate(next_source, next_update));
                            break;
                        }
                        if accumulated_targets + next_estimate > STATE_UPDATE_MAX_BATCH_TARGETS
                            && !accumulated_updates.is_empty()
                        {
                            pending_msg =
                                Some(MultiProofMessage::StateUpdate(next_source, next_update));
                            break;
                        }
                        accumulated_targets += next_estimate;
                        accumulated_updates.push((next_source, next_update));
                    }
                    Ok(other_msg) => {
                        pending_msg = Some(other_msg);
                        break;
                    }
                    Err(_) => break,
                }
            }

            assert_eq!(
                accumulated_updates.len(),
                1,
                "Second pre-block update should not merge with a different payload"
            );
            let (batched_source, batched_update) = accumulated_updates.remove(0);
            assert!(same_state_change_source(batched_source, source));
            assert!(batched_update.contains_key(&addr2));
            assert!(!batched_update.contains_key(&addr3));

            match pending_msg {
                Some(MultiProofMessage::StateUpdate(_, pending_update)) => {
                    assert!(pending_update.contains_key(&addr3));
                }
                other => panic!("Expected pending third pre-block update, got {:?}", other),
            }
        } else {
            panic!("Expected first StateUpdate");
        }
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
        let tx = task.state_root_message_sender();
        tx.send(MultiProofMessage::PrefetchProofs(targets1)).unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(targets2)).unwrap();
        tx.send(MultiProofMessage::StateUpdate(source, state_update1)).unwrap();
        tx.send(MultiProofMessage::StateUpdate(source, state_update2)).unwrap();
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

        let tx = task.state_root_message_sender();
        tx.send(MultiProofMessage::PrefetchProofs(prefetch1)).unwrap();
        tx.send(MultiProofMessage::StateUpdate(source, state_update)).unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(prefetch2.clone())).unwrap();

        let mut ctx = MultiproofBatchCtx::new(Instant::now());
        let mut batch_metrics = MultiproofBatchMetrics::default();

        // First message: Prefetch1 batches; StateUpdate becomes pending.
        let first = task.rx.recv().unwrap();
        assert!(matches!(first, MultiProofMessage::PrefetchProofs(_)));
        assert!(!task.process_multiproof_message(first, &mut ctx, &mut batch_metrics));
        let pending = ctx.pending_msg.take().expect("pending message captured");
        assert!(matches!(pending, MultiProofMessage::StateUpdate(_, _)));

        // Pending message should be handled before the next select loop.
        assert!(!task.process_multiproof_message(pending, &mut ctx, &mut batch_metrics));

        // The remaining message should still be Prefetch2 (ordering preserved).
        match task.rx.try_recv() {
            Ok(MultiProofMessage::PrefetchProofs(targets)) => {
                assert_eq!(targets.len(), 1);
                assert!(targets.contains_key(&prefetch_addr2));
            }
            other => panic!("Expected remaining PrefetchProofs2, got {:?}", other),
        }
    }

    /// Verifies that pending messages from a previous batch drain get full batching treatment.
    #[test]
    fn test_pending_messages_get_full_batching_treatment() {
        // Queue: [Prefetch1, State1, State2, State3, Prefetch2]
        //
        // Expected behavior:
        // 1. recv() → Prefetch1
        // 2. try_recv() → State1 is different type → pending = State1, break
        // 3. Process Prefetch1
        // 4. Next iteration: pending = State1 → process with batching
        // 5. try_recv() → State2 same type → merge
        // 6. try_recv() → State3 same type → merge
        // 7. try_recv() → Prefetch2 different type → pending = Prefetch2, break
        // 8. Process merged State (1+2+3)
        //
        // Without the state-machine fix, State1 would be processed alone (no batching).
        use alloy_evm::block::StateChangeSource;
        use revm_state::Account;

        let test_provider_factory = create_test_provider_factory();
        let task = create_test_state_root_task(test_provider_factory);

        let prefetch_addr1 = B256::random();
        let prefetch_addr2 = B256::random();
        let state_addr1 = alloy_primitives::Address::random();
        let state_addr2 = alloy_primitives::Address::random();
        let state_addr3 = alloy_primitives::Address::random();

        // Create Prefetch targets
        let mut prefetch1 = MultiProofTargets::default();
        prefetch1.insert(prefetch_addr1, HashSet::default());

        let mut prefetch2 = MultiProofTargets::default();
        prefetch2.insert(prefetch_addr2, HashSet::default());

        // Create StateUpdates
        let create_state_update = |addr: alloy_primitives::Address, balance: u64| {
            let mut state = EvmState::default();
            state.insert(
                addr,
                Account {
                    info: revm_state::AccountInfo {
                        balance: U256::from(balance),
                        nonce: 1,
                        code_hash: Default::default(),
                        code: Default::default(),
                    },
                    transaction_id: Default::default(),
                    storage: Default::default(),
                    status: revm_state::AccountStatus::Touched,
                },
            );
            state
        };

        let source = StateChangeSource::Transaction(42);

        // Queue: [Prefetch1, State1, State2, State3, Prefetch2]
        let tx = task.state_root_message_sender();
        tx.send(MultiProofMessage::PrefetchProofs(prefetch1.clone())).unwrap();
        tx.send(MultiProofMessage::StateUpdate(source, create_state_update(state_addr1, 100)))
            .unwrap();
        tx.send(MultiProofMessage::StateUpdate(source, create_state_update(state_addr2, 200)))
            .unwrap();
        tx.send(MultiProofMessage::StateUpdate(source, create_state_update(state_addr3, 300)))
            .unwrap();
        tx.send(MultiProofMessage::PrefetchProofs(prefetch2.clone())).unwrap();

        // Simulate the state-machine loop behavior
        let mut pending_msg: Option<MultiProofMessage> = None;

        // First iteration: recv() gets Prefetch1, drains until State1
        if let Ok(MultiProofMessage::PrefetchProofs(targets)) = task.rx.recv() {
            let mut merged_targets = targets;
            loop {
                match task.rx.try_recv() {
                    Ok(MultiProofMessage::PrefetchProofs(next_targets)) => {
                        merged_targets.extend(next_targets);
                    }
                    Ok(other_msg) => {
                        pending_msg = Some(other_msg);
                        break;
                    }
                    Err(_) => break,
                }
            }
            // Should have only Prefetch1 (State1 is different type)
            assert_eq!(merged_targets.len(), 1);
            assert!(merged_targets.contains_key(&prefetch_addr1));
        } else {
            panic!("Expected PrefetchProofs");
        }

        // Pending should be State1
        assert!(matches!(pending_msg, Some(MultiProofMessage::StateUpdate(_, _))));

        // Second iteration: process pending State1 WITH BATCHING
        // This is the key test - the pending message should drain State2 and State3
        if let Some(MultiProofMessage::StateUpdate(_src, first_update)) = pending_msg.take() {
            let mut merged_update = first_update;
            let mut num_batched = 1;

            loop {
                match task.rx.try_recv() {
                    Ok(MultiProofMessage::StateUpdate(_src, next_update)) => {
                        merged_update.extend(next_update);
                        num_batched += 1;
                    }
                    Ok(other_msg) => {
                        pending_msg = Some(other_msg);
                        break;
                    }
                    Err(_) => break,
                }
            }

            // THE KEY ASSERTION: pending State1 should have batched with State2 and State3
            assert_eq!(
                num_batched, 3,
                "Pending message should get full batching treatment and merge all 3 StateUpdates"
            );
            assert_eq!(merged_update.len(), 3, "Should have all 3 addresses in merged update");
            assert!(merged_update.contains_key(&state_addr1));
            assert!(merged_update.contains_key(&state_addr2));
            assert!(merged_update.contains_key(&state_addr3));
        } else {
            panic!("Expected pending StateUpdate");
        }

        // Pending should now be Prefetch2
        match pending_msg {
            Some(MultiProofMessage::PrefetchProofs(targets)) => {
                assert_eq!(targets.len(), 1);
                assert!(targets.contains_key(&prefetch_addr2));
            }
            _ => panic!("Prefetch2 was lost!"),
        }
    }
}
