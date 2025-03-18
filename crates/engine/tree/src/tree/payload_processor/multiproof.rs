//! Multiproof task related functionality.

use crate::tree::payload_processor::executor::WorkloadExecutor;
use alloy_evm::block::StateChangeSource;
use alloy_primitives::{keccak256, map::HashSet, B256};
use derive_more::derive::Deref;
use metrics::Histogram;
use reth_errors::ProviderError;
use reth_metrics::Metrics;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DatabaseProviderFactory, FactoryTx,
    StateCommitmentProvider,
};
use reth_revm::state::EvmState;
use reth_trie::{
    prefix_set::TriePrefixSetsMut, updates::TrieUpdatesSorted, HashedPostState,
    HashedPostStateSorted, HashedStorage, MultiProof, MultiProofTargets, TrieInput,
};
use reth_trie_parallel::{
    proof::ParallelProof,
    proof_task::{ProofTaskCtx, ProofTaskManager, ProofTaskManagerHandle},
};
use std::{
    collections::{BTreeMap, VecDeque},
    ops::DerefMut,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    time::{Duration, Instant},
};
use tracing::{debug, error, trace};

/// The size of proof targets chunk to spawn in one calculation.
const MULTIPROOF_TARGETS_CHUNK_SIZE: usize = 10;

/// A trie update that can be applied to sparse trie alongside the proofs for touched parts of the
/// state.
#[derive(Default, Debug)]
pub struct SparseTrieUpdate {
    /// The state update that was used to calculate the proof
    pub(crate) state: HashedPostState,
    /// The calculated multiproof
    pub(crate) multiproof: MultiProof,
}

impl SparseTrieUpdate {
    /// Returns true if the update is empty.
    pub(super) fn is_empty(&self) -> bool {
        self.state.is_empty() && self.multiproof.is_empty()
    }

    /// Construct update from multiproof.
    #[cfg(test)]
    pub(super) fn from_multiproof(multiproof: MultiProof) -> Self {
        Self { multiproof, ..Default::default() }
    }

    /// Extend update with contents of the other.
    pub(super) fn extend(&mut self, other: Self) {
        self.state.extend(other.state);
        self.multiproof.extend(other.multiproof);
    }
}

/// Common configuration for multi proof tasks
#[derive(Debug, Clone)]
pub(super) struct MultiProofConfig<Factory> {
    /// View over the state in the database.
    pub consistent_view: ConsistentDbView<Factory>,
    /// The sorted collection of cached in-memory intermediate trie nodes that
    /// can be reused for computation.
    pub nodes_sorted: Arc<TrieUpdatesSorted>,
    /// The sorted in-memory overlay hashed state.
    pub state_sorted: Arc<HashedPostStateSorted>,
    /// The collection of prefix sets for the computation. Since the prefix sets _always_
    /// invalidate the in-memory nodes, not all keys from `state_sorted` might be present here,
    /// if we have cached nodes for them.
    pub prefix_sets: Arc<TriePrefixSetsMut>,
}

impl<Factory> MultiProofConfig<Factory> {
    /// Creates a new state root config from the consistent view and the trie input.
    pub(super) fn new_from_input(
        consistent_view: ConsistentDbView<Factory>,
        input: TrieInput,
    ) -> Self {
        Self {
            consistent_view,
            nodes_sorted: Arc::new(input.nodes.into_sorted()),
            state_sorted: Arc::new(input.state.into_sorted()),
            prefix_sets: Arc::new(input.prefix_sets),
        }
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
    /// Proof calculation completed for a specific state update
    ProofCalculated(Box<ProofCalculated>),
    /// Error during proof calculation
    ProofCalculationError(ProviderError),
    /// Signals state update stream end.
    ///
    /// This is triggered by block execution, indicating that no additional state updates are
    /// expected.
    FinishedStateUpdates,
}

/// Message about completion of proof calculation for a specific state update
#[derive(Debug)]
pub(super) struct ProofCalculated {
    /// The index of this proof in the sequence of state updates
    sequence_number: u64,
    /// Sparse trie update
    update: SparseTrieUpdate,
    /// The time taken to calculate the proof.
    elapsed: Duration,
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
    fn next_sequence(&mut self) -> u64 {
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
pub(super) struct StateHookSender(Sender<MultiProofMessage>);

impl StateHookSender {
    pub(crate) const fn new(inner: Sender<MultiProofMessage>) -> Self {
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
            trace!(target: "engine::root", ?address, ?hashed_address, "Adding account to state update");

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

/// Input parameters for spawning a multiproof calculation.
#[derive(Debug)]
struct MultiproofInput<Factory> {
    config: MultiProofConfig<Factory>,
    source: Option<StateChangeSource>,
    hashed_state_update: HashedPostState,
    proof_targets: MultiProofTargets,
    proof_sequence_number: u64,
    state_root_message_sender: Sender<MultiProofMessage>,
}

/// Manages concurrent multiproof calculations.
/// Takes care of not having more calculations in flight than a given maximum
/// concurrency, further calculation requests are queued and spawn later, after
/// availability has been signaled.
#[derive(Debug)]
pub struct MultiproofManager<Factory: DatabaseProviderFactory> {
    /// Maximum number of concurrent calculations.
    max_concurrent: usize,
    /// Currently running calculations.
    inflight: usize,
    /// Queued calculations.
    pending: VecDeque<MultiproofInput<Factory>>,
    /// Executor for tasks
    executor: WorkloadExecutor,
    /// Sender to the storage proof task.
    storage_proof_task_handle: ProofTaskManagerHandle<FactoryTx<Factory>>,
    /// Metrics
    metrics: MultiProofTaskMetrics,
}

impl<Factory> MultiproofManager<Factory>
where
    Factory:
        DatabaseProviderFactory<Provider: BlockReader> + StateCommitmentProvider + Clone + 'static,
{
    /// Creates a new [`MultiproofManager`].
    fn new(
        executor: WorkloadExecutor,
        metrics: MultiProofTaskMetrics,
        storage_proof_task_handle: ProofTaskManagerHandle<FactoryTx<Factory>>,
    ) -> Self {
        let max_concurrent = executor.rayon_pool().current_num_threads();
        Self {
            pending: VecDeque::with_capacity(max_concurrent),
            max_concurrent,
            executor,
            inflight: 0,
            metrics,
            storage_proof_task_handle,
        }
    }

    /// Spawns a new multiproof calculation or enqueues it for later if
    /// `max_concurrent` are already inflight.
    fn spawn_or_queue(&mut self, input: MultiproofInput<Factory>) {
        // If there are no proof targets, we can just send an empty multiproof back immediately
        if input.proof_targets.is_empty() {
            debug!(
                sequence_number = input.proof_sequence_number,
                "No proof targets, sending empty multiproof back immediately"
            );
            let _ = input.state_root_message_sender.send(MultiProofMessage::EmptyProof {
                sequence_number: input.proof_sequence_number,
                state: input.hashed_state_update,
            });
            return
        }

        if self.inflight >= self.max_concurrent {
            self.pending.push_back(input);
            self.metrics.pending_multiproofs_histogram.record(self.pending.len() as f64);
            return;
        }

        self.spawn_multiproof(input);
    }

    /// Signals that a multiproof calculation has finished and there's room to
    /// spawn a new calculation if needed.
    fn on_calculation_complete(&mut self) {
        self.inflight = self.inflight.saturating_sub(1);
        self.metrics.inflight_multiproofs_histogram.record(self.inflight as f64);

        if let Some(input) = self.pending.pop_front() {
            self.metrics.pending_multiproofs_histogram.record(self.pending.len() as f64);
            self.spawn_multiproof(input);
        }
    }

    /// Spawns a single multiproof calculation task.
    fn spawn_multiproof(
        &mut self,
        MultiproofInput {
            config,
            source,
            hashed_state_update,
            proof_targets,
            proof_sequence_number,
            state_root_message_sender,
        }: MultiproofInput<Factory>,
    ) {
        let to_storage_proof_task = self.storage_proof_task_handle.sender();

        self.executor.spawn_blocking(move || {
            let account_targets = proof_targets.len();
            let storage_targets = proof_targets.values().map(|slots| slots.len()).sum::<usize>();

            trace!(
                target: "engine::root",
                proof_sequence_number,
                ?proof_targets,
                account_targets,
                storage_targets,
                "Starting multiproof calculation",
            );
            let start = Instant::now();
            let result = ParallelProof::new(
                config.consistent_view,
                config.nodes_sorted,
                config.state_sorted,
                config.prefix_sets,
                to_storage_proof_task.clone(),
            )
            .with_branch_node_masks(true)
            .multiproof(proof_targets);
            let elapsed = start.elapsed();
            trace!(
                target: "engine::root",
                proof_sequence_number,
                ?elapsed,
                ?source,
                account_targets,
                storage_targets,
                "Multiproof calculated",
            );

            match result {
                Ok(proof) => {
                    let _ = state_root_message_sender.send(MultiProofMessage::ProofCalculated(
                        Box::new(ProofCalculated {
                            sequence_number: proof_sequence_number,
                            update: SparseTrieUpdate {
                                state: hashed_state_update,
                                multiproof: proof,
                            },
                            elapsed,
                        }),
                    ));
                }
                Err(error) => {
                    let _ = state_root_message_sender
                        .send(MultiProofMessage::ProofCalculationError(error.into()));
                }
            }
        });

        self.inflight += 1;
        self.metrics.inflight_multiproofs_histogram.record(self.inflight as f64);
    }
}

#[derive(Metrics, Clone)]
#[metrics(scope = "tree.root")]
pub(crate) struct MultiProofTaskMetrics {
    /// Histogram of inflight multiproofs.
    pub inflight_multiproofs_histogram: Histogram,
    /// Histogram of pending multiproofs.
    pub pending_multiproofs_histogram: Histogram,

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
}

/// Standalone task that receives a transaction state stream and updates relevant
/// data structures to calculate state root.
///
/// It is responsible of  initializing a blinded sparse trie and subscribe to
/// transaction state stream. As it receives transaction execution results, it
/// fetches the proofs for relevant accounts from the database and reveal them
/// to the tree.
/// Then it updates relevant leaves according to the result of the transaction.
/// This feeds updates to the sparse trie task.
#[derive(Debug)]
pub(super) struct MultiProofTask<Factory: DatabaseProviderFactory> {
    /// Task configuration.
    config: MultiProofConfig<Factory>,
    /// Receiver for state root related messages.
    rx: Receiver<MultiProofMessage>,
    /// Sender for state root related messages.
    tx: Sender<MultiProofMessage>,
    /// Sender for state updates emitted by this type.
    to_sparse_trie: Sender<SparseTrieUpdate>,
    /// Proof targets that have been already fetched.
    fetched_proof_targets: MultiProofTargets,
    /// Proof sequencing handler.
    proof_sequencer: ProofSequencer,
    /// Manages calculation of multiproofs.
    multiproof_manager: MultiproofManager<Factory>,
    /// multi proof task metrics
    metrics: MultiProofTaskMetrics,
}

impl<Factory> MultiProofTask<Factory>
where
    Factory:
        DatabaseProviderFactory<Provider: BlockReader> + StateCommitmentProvider + Clone + 'static,
{
    /// Creates a new multi proof task with the unified message channel
    pub(super) fn new(
        config: MultiProofConfig<Factory>,
        executor: WorkloadExecutor,
        to_sparse_trie: Sender<SparseTrieUpdate>,
    ) -> Self {
        let (tx, rx) = channel();
        let metrics = MultiProofTaskMetrics::default();

        // Create and spawn the storage proof task
        let task_ctx = ProofTaskCtx::new(config.nodes_sorted.clone(), config.state_sorted.clone());
        let max_concurrency = 256;
        let proof_task = ProofTaskManager::new(
            executor.handle().clone(),
            config.consistent_view.clone(),
            task_ctx,
            max_concurrency,
        );

        // get proof task handle
        let proof_task_handle = proof_task.handle();

        // spawn the proof task
        executor.spawn_blocking(move || {
            if let Err(err) = proof_task.run() {
                // At least log if there is an error at any point
                tracing::error!(
                    target: "engine::root",
                    ?err,
                    "Storage proof task returned an error"
                );
            }
        });

        Self {
            config,
            rx,
            tx,
            to_sparse_trie,
            fetched_proof_targets: Default::default(),
            proof_sequencer: ProofSequencer::default(),
            multiproof_manager: MultiproofManager::new(
                executor,
                metrics.clone(),
                proof_task_handle,
            ),
            metrics,
        }
    }

    /// Returns a [`Sender`] that can be used to send arbitrary [`MultiProofMessage`]s to this task.
    pub(super) fn state_root_message_sender(&self) -> Sender<MultiProofMessage> {
        self.tx.clone()
    }

    /// Handles request for proof prefetch.
    ///
    /// Returns a number of proofs that were spawned.
    fn on_prefetch_proof(&mut self, targets: MultiProofTargets) -> u64 {
        let proof_targets = self.get_prefetch_proof_targets(targets);
        self.fetched_proof_targets.extend_ref(&proof_targets);

        self.metrics.prefetch_proof_targets_accounts_histogram.record(proof_targets.len() as f64);
        self.metrics
            .prefetch_proof_targets_storages_histogram
            .record(proof_targets.values().map(|slots| slots.len()).sum::<usize>() as f64);

        // Process proof targets in chunks.
        let mut chunks = 0;
        for proof_targets_chunk in proof_targets.chunks(MULTIPROOF_TARGETS_CHUNK_SIZE) {
            self.multiproof_manager.spawn_or_queue(MultiproofInput {
                config: self.config.clone(),
                source: None,
                hashed_state_update: Default::default(),
                proof_targets: proof_targets_chunk,
                proof_sequence_number: self.proof_sequencer.next_sequence(),
                state_root_message_sender: self.tx.clone(),
            });
            chunks += 1;
        }
        self.metrics.prefetch_proof_chunks_histogram.record(chunks as f64);

        chunks
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
        debug!(
            target: "engine::root",
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
                continue
            };

            let prev_target_storage_len = target_storage.len();

            // keep only the storage slots that have not been fetched yet
            //
            // we already removed subsets, so this should only remove duplicates
            target_storage.retain(|slot| !fetched_storage.contains(slot));

            duplicates += prev_target_storage_len - target_storage.len();
        }

        if duplicates > 0 {
            trace!(target: "engine::root", duplicates, "Removed duplicate prefetch proof targets");
        }

        targets
    }

    /// Handles state updates.
    ///
    /// Returns a number of proofs that were spawned.
    fn on_state_update(&mut self, source: StateChangeSource, update: EvmState) -> u64 {
        let hashed_state_update = evm_state_to_hashed_post_state(update);
        // Split the state update into already fetched and not fetched according to the proof
        // targets.
        let (fetched_state_update, not_fetched_state_update) =
            hashed_state_update.partition_by_targets(&self.fetched_proof_targets);

        let mut state_updates = 0;
        // If there are any accounts or storage slots that we already fetched the proofs for,
        // send them immediately, as they don't require spawning any additional multiproofs.
        if !fetched_state_update.is_empty() {
            let _ = self.tx.send(MultiProofMessage::EmptyProof {
                sequence_number: self.proof_sequencer.next_sequence(),
                state: fetched_state_update,
            });
            state_updates += 1;
        }

        // Process state updates in chunks.
        let mut chunks = 0;
        let mut spawned_proof_targets = MultiProofTargets::default();
        for chunk in not_fetched_state_update.chunks(MULTIPROOF_TARGETS_CHUNK_SIZE) {
            let proof_targets = get_proof_targets(&chunk, &self.fetched_proof_targets);
            spawned_proof_targets.extend_ref(&proof_targets);

            self.multiproof_manager.spawn_or_queue(MultiproofInput {
                config: self.config.clone(),
                source: Some(source),
                hashed_state_update: chunk,
                proof_targets,
                proof_sequence_number: self.proof_sequencer.next_sequence(),
                state_root_message_sender: self.tx.clone(),
            });
            chunks += 1;
        }

        self.metrics
            .state_update_proof_targets_accounts_histogram
            .record(spawned_proof_targets.len() as f64);
        self.metrics
            .state_update_proof_targets_storages_histogram
            .record(spawned_proof_targets.values().map(|slots| slots.len()).sum::<usize>() as f64);
        self.metrics.state_update_proof_chunks_histogram.record(chunks as f64);

        self.fetched_proof_targets.extend(spawned_proof_targets);

        state_updates + chunks
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
    ///    [`MultiproofManager::spawn_or_queue`].
    ///    * If the list of proof targets is empty, the [`MultiProofMessage::EmptyProof`] message is
    ///      sent back to this task along with the original state update.
    ///    * Otherwise, the multiproof is calculated and the [`MultiProofMessage::ProofCalculated`]
    ///      message is sent back to this task along with the resulting multiproof, proof targets
    ///      and original state update.
    /// 3. Either [`MultiProofMessage::EmptyProof`] or [`MultiProofMessage::ProofCalculated`] is
    ///    received.
    ///    * The multiproof is added to the (proof sequencer)[`ProofSequencer`].
    ///    * If the proof sequencer has a contiguous sequence of multiproofs in the same order as
    ///      state updates arrived (i.e. transaction order), such sequence is returned.
    /// 4. Once there's a sequence of contiguous multiproofs along with the proof targets and state
    ///    updates associated with them, a [`SparseTrieUpdate`] is generated and sent to the sparse
    ///    trie task.
    /// 5. Steps above are repeated until this task receives a
    ///    [`MultiProofMessage::FinishedStateUpdates`].
    ///    * Once this message is received, on every [`MultiProofMessage::EmptyProof`] and
    ///      [`MultiProofMessage::ProofCalculated`] message, we check if there are any proofs are
    ///      currently being calculated, or if there are any pending proofs in the proof sequencer
    ///      left to be revealed by checking the pending tasks.
    /// 6. This task exits after all pending proofs are processed.
    pub(crate) fn run(mut self) {
        // TODO convert those into fields
        let mut prefetch_proofs_requested = 0;
        let mut state_update_proofs_requested = 0;
        let mut proofs_processed = 0;

        let mut updates_finished = false;

        // Timestamp before the first state update or prefetch was received
        let start = Instant::now();

        // Timestamp when the first state update or prefetch was received
        let mut first_update_time = None;
        // Timestamp when the last state update was received
        let mut last_update_time = None;

        loop {
            trace!(target: "engine::root", "entering main channel receiving loop");
            match self.rx.recv() {
                Ok(message) => match message {
                    MultiProofMessage::PrefetchProofs(targets) => {
                        trace!(target: "engine::root", "processing MultiProofMessage::PrefetchProofs");
                        if first_update_time.is_none() {
                            // record the wait time
                            self.metrics
                                .first_update_wait_time_histogram
                                .record(start.elapsed().as_secs_f64());
                            first_update_time = Some(Instant::now());
                            debug!(target: "engine::root", "Started state root calculation");
                        }

                        let account_targets = targets.len();
                        let storage_targets =
                            targets.values().map(|slots| slots.len()).sum::<usize>();
                        prefetch_proofs_requested += self.on_prefetch_proof(targets);
                        debug!(
                            target: "engine::root",
                            account_targets,
                            storage_targets,
                            prefetch_proofs_requested,
                            "Prefetching proofs"
                        );
                    }
                    MultiProofMessage::StateUpdate(source, update) => {
                        trace!(target: "engine::root", "processing MultiProofMessage::StateUpdate");
                        if first_update_time.is_none() {
                            // record the wait time
                            self.metrics
                                .first_update_wait_time_histogram
                                .record(start.elapsed().as_secs_f64());
                            first_update_time = Some(Instant::now());
                            debug!(target: "engine::root", "Started state root calculation");
                        }
                        last_update_time = Some(Instant::now());

                        let len = update.len();
                        state_update_proofs_requested += self.on_state_update(source, update);
                        debug!(
                            target: "engine::root",
                            ?source,
                            len,
                            ?state_update_proofs_requested,
                            "Received new state update"
                        );
                    }
                    MultiProofMessage::FinishedStateUpdates => {
                        trace!(target: "engine::root", "processing MultiProofMessage::FinishedStateUpdates");
                        updates_finished = true;
                        if self.is_done(
                            proofs_processed,
                            state_update_proofs_requested,
                            prefetch_proofs_requested,
                            updates_finished,
                        ) {
                            debug!(
                                target: "engine::root",
                                "State updates finished and all proofs processed, ending calculation"
                            );
                            break
                        }
                    }
                    MultiProofMessage::EmptyProof { sequence_number, state } => {
                        trace!(target: "engine::root", "processing MultiProofMessage::EmptyProof");

                        proofs_processed += 1;

                        if let Some(combined_update) = self.on_proof(
                            sequence_number,
                            SparseTrieUpdate { state, multiproof: MultiProof::default() },
                        ) {
                            let _ = self.to_sparse_trie.send(combined_update);
                        }

                        if self.is_done(
                            proofs_processed,
                            state_update_proofs_requested,
                            prefetch_proofs_requested,
                            updates_finished,
                        ) {
                            debug!(
                                target: "engine::root",
                                "State updates finished and all proofs processed, ending calculation"
                            );
                            break
                        }
                    }
                    MultiProofMessage::ProofCalculated(proof_calculated) => {
                        trace!(target: "engine::root", "processing
        MultiProofMessage::ProofCalculated");

                        // we increment proofs_processed for both state updates and prefetches,
                        // because both are used for the root termination condition.
                        proofs_processed += 1;

                        self.metrics
                            .proof_calculation_duration_histogram
                            .record(proof_calculated.elapsed);

                        debug!(
                            target: "engine::root",
                            sequence = proof_calculated.sequence_number,
                            total_proofs = proofs_processed,
                            "Processing calculated proof"
                        );

                        self.multiproof_manager.on_calculation_complete();

                        if let Some(combined_update) =
                            self.on_proof(proof_calculated.sequence_number, proof_calculated.update)
                        {
                            let _ = self.to_sparse_trie.send(combined_update);
                        }

                        if self.is_done(
                            proofs_processed,
                            state_update_proofs_requested,
                            prefetch_proofs_requested,
                            updates_finished,
                        ) {
                            debug!(
                                target: "engine::root",
                                "State updates finished and all proofs processed, ending calculation");
                            break
                        }
                    }
                    MultiProofMessage::ProofCalculationError(err) => {
                        error!(
                            target: "engine::root",
                            ?err,
                            "proof calculation error"
                        );
                        return
                    }
                },
                Err(_) => {
                    // this means our internal message channel is closed, which shouldn't happen
                    // in normal operation since we hold both ends
                    error!(
                        target: "engine::root",
                        "Internal message channel closed unexpectedly"
                    );
                }
            }
        }

        debug!(
            target: "engine::root",
            total_updates = state_update_proofs_requested,
            total_proofs = proofs_processed,
            total_time = ?first_update_time.map(|t|t.elapsed()),
            time_from_last_update = ?last_update_time.map(|t|t.elapsed()),
            "All proofs processed, ending calculation"
        );

        // update total metrics on finish
        self.metrics.state_updates_received_histogram.record(state_update_proofs_requested as f64);
        self.metrics.proofs_processed_histogram.record(proofs_processed as f64);
        if let Some(total_time) = first_update_time.map(|t| t.elapsed()) {
            self.metrics.multiproof_task_total_duration_histogram.record(total_time);
        }
    }
}

/// Returns accounts only with those storages that were not already fetched, and
/// if there are no such storages and the account itself was already fetched, the
/// account shouldn't be included.
fn get_proof_targets(
    state_update: &HashedPostState,
    fetched_proof_targets: &MultiProofTargets,
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
        let mut changed_slots = storage
            .storage
            .keys()
            .filter(|slot| !fetched.is_some_and(|f| f.contains(*slot)))
            .peekable();

        if changed_slots.peek().is_some() {
            targets.entry(*hashed_address).or_default().extend(changed_slots);
        }
    }

    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::map::B256Set;
    use reth_provider::{providers::ConsistentDbView, test_utils::create_test_provider_factory};
    use reth_trie::TrieInput;
    use revm_primitives::{B256, U256};
    use std::sync::Arc;

    fn create_state_root_config<F>(factory: F, input: TrieInput) -> MultiProofConfig<F>
    where
        F: DatabaseProviderFactory<Provider: BlockReader>
            + StateCommitmentProvider
            + Clone
            + 'static,
    {
        let consistent_view = ConsistentDbView::new(factory, None);
        let nodes_sorted = Arc::new(input.nodes.clone().into_sorted());
        let state_sorted = Arc::new(input.state.clone().into_sorted());
        let prefix_sets = Arc::new(input.prefix_sets);

        MultiProofConfig { consistent_view, nodes_sorted, state_sorted, prefix_sets }
    }

    fn create_test_state_root_task<F>(factory: F) -> MultiProofTask<F>
    where
        F: DatabaseProviderFactory<Provider: BlockReader>
            + StateCommitmentProvider
            + Clone
            + 'static,
    {
        let executor = WorkloadExecutor::with_num_cpu_threads(2);
        let config = create_state_root_config(factory, TrieInput::default());
        let channel = channel();

        MultiProofTask::new(config, executor, channel.0)
    }

    #[test]
    fn test_add_proof_in_sequence() {
        let mut sequencer = ProofSequencer::default();
        let proof1 = MultiProof::default();
        let proof2 = MultiProof::default();
        sequencer.next_sequence = 2;

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof1));
        assert_eq!(ready.len(), 1);
        assert!(!sequencer.has_pending());

        let ready = sequencer.add_proof(1, SparseTrieUpdate::from_multiproof(proof2));
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

        let ready = sequencer.add_proof(2, SparseTrieUpdate::from_multiproof(proof3));
        assert_eq!(ready.len(), 0);
        assert!(sequencer.has_pending());

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof1));
        assert_eq!(ready.len(), 1);
        assert!(sequencer.has_pending());

        let ready = sequencer.add_proof(1, SparseTrieUpdate::from_multiproof(proof2));
        assert_eq!(ready.len(), 2);
        assert!(!sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_with_gaps() {
        let mut sequencer = ProofSequencer::default();
        let proof1 = MultiProof::default();
        let proof3 = MultiProof::default();
        sequencer.next_sequence = 3;

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof1));
        assert_eq!(ready.len(), 1);

        let ready = sequencer.add_proof(2, SparseTrieUpdate::from_multiproof(proof3));
        assert_eq!(ready.len(), 0);
        assert!(sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_duplicate_sequence() {
        let mut sequencer = ProofSequencer::default();
        let proof1 = MultiProof::default();
        let proof2 = MultiProof::default();

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof1));
        assert_eq!(ready.len(), 1);

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proof2));
        assert_eq!(ready.len(), 0);
        assert!(!sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_batch_processing() {
        let mut sequencer = ProofSequencer::default();
        let proofs: Vec<_> = (0..5).map(|_| MultiProof::default()).collect();
        sequencer.next_sequence = 5;

        sequencer.add_proof(4, SparseTrieUpdate::from_multiproof(proofs[4].clone()));
        sequencer.add_proof(2, SparseTrieUpdate::from_multiproof(proofs[2].clone()));
        sequencer.add_proof(1, SparseTrieUpdate::from_multiproof(proofs[1].clone()));
        sequencer.add_proof(3, SparseTrieUpdate::from_multiproof(proofs[3].clone()));

        let ready = sequencer.add_proof(0, SparseTrieUpdate::from_multiproof(proofs[0].clone()));
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

        let targets = get_proof_targets(&state, &fetched);

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

        let targets = get_proof_targets(&state, &fetched);

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

        let targets = get_proof_targets(&state, &fetched);

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

        let targets = get_proof_targets(&state, &fetched);

        // should not include the already fetched storage slot
        let target_slots = &targets[addr];
        assert!(!target_slots.contains(&fetched_slot));
        assert_eq!(target_slots.len(), storage.storage.len() - 1);
    }

    #[test]
    fn test_get_proof_targets_empty_state() {
        let state = HashedPostState::default();
        let fetched = MultiProofTargets::default();

        let targets = get_proof_targets(&state, &fetched);

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

        let targets = get_proof_targets(&state, &fetched);

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

        let targets = get_proof_targets(&state, &fetched);

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
        targets.insert(addr1, vec![slot1].into_iter().collect());
        targets.insert(addr2, vec![slot2].into_iter().collect());

        let prefetch_proof_targets =
            test_state_root_task.get_prefetch_proof_targets(targets.clone());

        // check that the prefetch proof targets are the same because there are no fetched proof
        // targets yet
        assert_eq!(prefetch_proof_targets, targets);

        // add a different addr and slot to fetched proof targets
        let addr3 = B256::random();
        let slot3 = B256::random();
        test_state_root_task.fetched_proof_targets.insert(addr3, vec![slot3].into_iter().collect());

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
        targets.insert(addr1, vec![slot1].into_iter().collect());
        targets.insert(addr2, vec![slot2].into_iter().collect());

        // add a subset of the first target to fetched proof targets
        test_state_root_task.fetched_proof_targets.insert(addr1, vec![slot1].into_iter().collect());

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
            vec![slot3].into_iter().collect::<B256Set>()
        );
        assert!(prefetch_proof_targets.contains_key(&addr2));
        assert_eq!(
            *prefetch_proof_targets.get(&addr2).unwrap(),
            vec![slot2].into_iter().collect::<B256Set>()
        );
    }
}
