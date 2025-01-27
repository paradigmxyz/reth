//! State root task related functionality.

use alloy_primitives::map::HashSet;
use derive_more::derive::Deref;
use rayon::iter::{ParallelBridge, ParallelIterator};
use reth_errors::{ProviderError, ProviderResult};
use reth_evm::system_calls::OnStateHook;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory,
    StateCommitmentProvider,
};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory,
    prefix_set::TriePrefixSetsMut,
    proof::ProofBlindedProviderFactory,
    trie_cursor::InMemoryTrieCursorFactory,
    updates::{TrieUpdates, TrieUpdatesSorted},
    HashedPostState, HashedPostStateSorted, HashedStorage, MultiProof, MultiProofTargets, Nibbles,
    TrieInput,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use reth_trie_parallel::{proof::ParallelProof, root::ParallelStateRootError};
use reth_trie_sparse::{
    blinded::{BlindedProvider, BlindedProviderFactory},
    errors::{SparseStateTrieResult, SparseTrieErrorKind},
    SparseStateTrie,
};
use revm_primitives::{keccak256, EvmState, B256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        mpsc::{self, channel, Receiver, Sender},
        Arc,
    },
    time::{Duration, Instant},
};
use tracing::{debug, error, trace};

/// The level below which the sparse trie hashes are calculated in [`update_sparse_trie`].
const SPARSE_TRIE_INCREMENTAL_LEVEL: usize = 2;

/// Determines the size of the thread pool to be used in [`StateRootTask`].
/// It should be at least three, one for multiproof calculations  plus two to be
/// used internally in [`StateRootTask`].
///
/// NOTE: this value can be greater than the available cores in the host, it
/// represents the maximum number of threads that can be handled by the pool.
pub(crate) fn thread_pool_size() -> usize {
    std::thread::available_parallelism().map_or(3, |num| (num.get() / 2).max(3))
}

/// Outcome of the state root computation, including the state root itself with
/// the trie updates and the total time spent.
#[derive(Debug)]
pub struct StateRootComputeOutcome {
    /// The computed state root and trie updates
    pub state_root: (B256, TrieUpdates),
    /// The total time spent calculating the state root
    pub total_time: Duration,
    /// The time spent calculating the state root since the last state update
    pub time_from_last_update: Duration,
}

/// A trie update that can be applied to sparse trie alongside the proofs for touched parts of the
/// state.
#[derive(Default, Debug)]
pub struct SparseTrieUpdate {
    /// The state update that was used to calculate the proof
    state: HashedPostState,
    /// The proof targets
    targets: MultiProofTargets,
    /// The calculated multiproof
    multiproof: MultiProof,
}

impl SparseTrieUpdate {
    /// Construct update from multiproof.
    pub fn from_multiproof(multiproof: MultiProof) -> Self {
        Self { multiproof, ..Default::default() }
    }

    /// Extend update with contents of the other.
    pub fn extend(&mut self, other: Self) {
        self.state.extend(other.state);
        extend_multi_proof_targets(&mut self.targets, other.targets);
        self.multiproof.extend(other.multiproof);
    }
}

/// Result of the state root calculation
pub(crate) type StateRootResult = Result<StateRootComputeOutcome, ParallelStateRootError>;

/// Handle to a spawned state root task.
#[derive(Debug)]
pub struct StateRootHandle {
    /// Channel for receiving the final result.
    rx: mpsc::Receiver<StateRootResult>,
}

impl StateRootHandle {
    /// Creates a new handle from a receiver.
    pub(crate) const fn new(rx: mpsc::Receiver<StateRootResult>) -> Self {
        Self { rx }
    }

    /// Waits for the state root calculation to complete.
    pub fn wait_for_result(self) -> StateRootResult {
        self.rx.recv().expect("state root task was dropped without sending result")
    }
}

/// Common configuration for state root tasks
#[derive(Debug, Clone)]
pub struct StateRootConfig<Factory> {
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

impl<Factory> StateRootConfig<Factory> {
    /// Creates a new state root config from the consistent view and the trie input.
    pub fn new_from_input(consistent_view: ConsistentDbView<Factory>, input: TrieInput) -> Self {
        Self {
            consistent_view,
            nodes_sorted: Arc::new(input.nodes.into_sorted()),
            state_sorted: Arc::new(input.state.into_sorted()),
            prefix_sets: Arc::new(input.prefix_sets),
        }
    }
}

/// Messages used internally by the state root task
#[derive(Debug)]
pub enum StateRootMessage {
    /// Prefetch proof targets
    PrefetchProofs(MultiProofTargets),
    /// New state update from transaction execution
    StateUpdate(EvmState),
    /// Proof calculation completed for a specific state update
    ProofCalculated(Box<ProofCalculated>),
    /// Error during proof calculation
    ProofCalculationError(ProviderError),
    /// State root calculation completed
    RootCalculated {
        /// Final state root.
        state_root: B256,
        /// Trie updates.
        trie_updates: TrieUpdates,
        /// The number of time sparse trie was updated.
        iterations: u64,
    },
    /// Error during state root calculation
    RootCalculationError(ParallelStateRootError),
    /// Signals state update stream end.
    FinishedStateUpdates,
}

/// Message about completion of proof calculation for a specific state update
#[derive(Debug)]
pub struct ProofCalculated {
    /// The index of this proof in the sequence of state updates
    sequence_number: u64,
    /// Sparse trie update
    update: SparseTrieUpdate,
    /// The source of the proof fetch, whether it was requested as a prefetch or as a result of a
    /// state update.
    source: ProofFetchSource,
}

impl ProofCalculated {
    /// Returns true if the proof was calculated as a result of a state update.
    pub(crate) const fn is_from_state_update(&self) -> bool {
        matches!(self.source, ProofFetchSource::StateUpdate)
    }
}

/// Whether or not a proof was fetched due to a state update, or due to a prefetch command.
#[derive(Debug)]
pub enum ProofFetchSource {
    /// The proof was fetched due to a prefetch command.
    Prefetch,
    /// The proof was fetched due to a state update.
    StateUpdate,
}

/// Handle to track proof calculation ordering
#[derive(Debug, Default)]
pub(crate) struct ProofSequencer {
    /// The next proof sequence number to be produced.
    next_sequence: u64,
    /// The next sequence number expected to be delivered.
    next_to_deliver: u64,
    /// Buffer for out-of-order proofs and corresponding state updates
    pending_proofs: BTreeMap<u64, SparseTrieUpdate>,
}

impl ProofSequencer {
    /// Creates a new proof sequencer
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Gets the next sequence number and increments the counter
    pub(crate) fn next_sequence(&mut self) -> u64 {
        let seq = self.next_sequence;
        self.next_sequence += 1;
        seq
    }

    /// Adds a proof with the corresponding state update and returns all sequential proofs and state
    /// updates if we have a continuous sequence
    pub(crate) fn add_proof(
        &mut self,
        sequence: u64,
        update: SparseTrieUpdate,
    ) -> Vec<SparseTrieUpdate> {
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

/// A wrapper for the sender that signals completion when dropped
#[derive(Deref, Debug)]
pub struct StateHookSender(Sender<StateRootMessage>);

impl StateHookSender {
    pub(crate) const fn new(inner: Sender<StateRootMessage>) -> Self {
        Self(inner)
    }
}

impl Drop for StateHookSender {
    fn drop(&mut self) {
        // Send completion signal when the sender is dropped
        let _ = self.0.send(StateRootMessage::FinishedStateUpdates);
    }
}

fn evm_state_to_hashed_post_state(update: EvmState) -> HashedPostState {
    let mut hashed_state = HashedPostState::default();

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
                .filter_map(|(slot, value)| {
                    value.is_changed().then(|| (keccak256(B256::from(slot)), value.present_value))
                })
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
    config: StateRootConfig<Factory>,
    hashed_state_update: HashedPostState,
    proof_targets: MultiProofTargets,
    proof_sequence_number: u64,
    state_root_message_sender: Sender<StateRootMessage>,
    source: ProofFetchSource,
}

/// Manages concurrent multiproof calculations.
/// Takes care of not having more calculations in flight than a given thread
/// pool size, further calculation requests are queued and spawn later, after
/// availability has been signaled.
#[derive(Debug)]
struct MultiproofManager<Factory> {
    /// Maximum number of concurrent calculations.
    max_concurrent: usize,
    /// Currently running calculations.
    inflight: usize,
    /// Queued calculations.
    pending: VecDeque<MultiproofInput<Factory>>,
    /// Thread pool to spawn multiproof calculations.
    thread_pool: Arc<rayon::ThreadPool>,
}

impl<Factory> MultiproofManager<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader>
        + StateCommitmentProvider
        + Clone
        + Send
        + Sync
        + 'static,
{
    /// Creates a new [`MultiproofManager`].
    fn new(thread_pool: Arc<rayon::ThreadPool>, thread_pool_size: usize) -> Self {
        // we keep 2 threads to be used internally by [`StateRootTask`]
        let max_concurrent = thread_pool_size.saturating_sub(2);
        debug_assert!(max_concurrent != 0);
        Self {
            thread_pool,
            max_concurrent,
            inflight: 0,
            pending: VecDeque::with_capacity(max_concurrent),
        }
    }

    /// Spawns a new multiproof calculation or enqueues it for later if
    /// `max_concurrent` are already inflight.
    fn spawn_or_queue(&mut self, input: MultiproofInput<Factory>) {
        if self.inflight >= self.max_concurrent {
            self.pending.push_back(input);
            return;
        }

        self.spawn_multiproof(input);
    }

    /// Signals that a multiproof calculation has finished and there's room to
    /// spawn a new calculation if needed.
    fn on_calculation_complete(&mut self) {
        self.inflight = self.inflight.saturating_sub(1);

        if let Some(input) = self.pending.pop_front() {
            self.spawn_multiproof(input);
        }
    }

    /// Spawns a multiproof calculation.
    fn spawn_multiproof(&mut self, input: MultiproofInput<Factory>) {
        let MultiproofInput {
            config,
            hashed_state_update,
            proof_targets,
            proof_sequence_number,
            state_root_message_sender,
            source,
        } = input;
        let thread_pool = self.thread_pool.clone();

        self.thread_pool.spawn(move || {
            trace!(
                target: "engine::root",
                proof_sequence_number,
                ?proof_targets,
                "Starting multiproof calculation",
            );
            let start = Instant::now();
            let result = calculate_multiproof(thread_pool, config, proof_targets.clone());
            trace!(
                target: "engine::root",
                proof_sequence_number,
                elapsed = ?start.elapsed(),
                "Multiproof calculated",
            );

            match result {
                Ok(proof) => {
                    let _ = state_root_message_sender.send(StateRootMessage::ProofCalculated(
                        Box::new(ProofCalculated {
                            sequence_number: proof_sequence_number,
                            update: SparseTrieUpdate {
                                state: hashed_state_update,
                                targets: proof_targets,
                                multiproof: proof,
                            },
                            source,
                        }),
                    ));
                }
                Err(error) => {
                    let _ = state_root_message_sender
                        .send(StateRootMessage::ProofCalculationError(error));
                }
            }
        });

        self.inflight += 1;
    }
}

/// Standalone task that receives a transaction state stream and updates relevant
/// data structures to calculate state root.
///
/// It is responsible of  initializing a blinded sparse trie and subscribe to
/// transaction state stream. As it receives transaction execution results, it
/// fetches the proofs for relevant accounts from the database and reveal them
/// to the tree.
/// Then it updates relevant leaves according to the result of the transaction.
#[derive(Debug)]
pub struct StateRootTask<Factory> {
    /// Task configuration.
    config: StateRootConfig<Factory>,
    /// Receiver for state root related messages.
    rx: Receiver<StateRootMessage>,
    /// Sender for state root related messages.
    tx: Sender<StateRootMessage>,
    /// Proof targets that have been already fetched.
    fetched_proof_targets: MultiProofTargets,
    /// Proof sequencing handler.
    proof_sequencer: ProofSequencer,
    /// Reference to the shared thread pool for parallel proof generation.
    thread_pool: Arc<rayon::ThreadPool>,
    /// Manages calculation of multiproofs.
    multiproof_manager: MultiproofManager<Factory>,
}

impl<Factory> StateRootTask<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader>
        + StateCommitmentProvider
        + Clone
        + Send
        + Sync
        + 'static,
{
    /// Creates a new state root task with the unified message channel
    pub fn new(config: StateRootConfig<Factory>, thread_pool: Arc<rayon::ThreadPool>) -> Self {
        let (tx, rx) = channel();
        Self {
            config,
            rx,
            tx,
            fetched_proof_targets: Default::default(),
            proof_sequencer: ProofSequencer::new(),
            thread_pool: thread_pool.clone(),
            multiproof_manager: MultiproofManager::new(thread_pool, thread_pool_size()),
        }
    }

    /// Returns a [`StateHookSender`] that can be used to send state updates to this task.
    pub fn state_hook_sender(&self) -> StateHookSender {
        StateHookSender::new(self.tx.clone())
    }

    /// Returns a state hook to be used to send state updates to this task.
    pub fn state_hook(&self) -> impl OnStateHook {
        let state_hook = self.state_hook_sender();

        move |state: &EvmState| {
            if let Err(error) = state_hook.send(StateRootMessage::StateUpdate(state.clone())) {
                error!(target: "engine::root", ?error, "Failed to send state update");
            }
        }
    }

    /// Spawns the state root task and returns a handle to await its result.
    pub fn spawn(self) -> StateRootHandle {
        let sparse_trie_tx =
            Self::spawn_sparse_trie(self.thread_pool.clone(), self.config.clone(), self.tx.clone());
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("State Root Task".to_string())
            .spawn(move || {
                debug!(target: "engine::tree", "Starting state root task");

                let result = self.run(sparse_trie_tx);
                let _ = tx.send(result);
            })
            .expect("failed to spawn state root thread");

        StateRootHandle::new(rx)
    }

    /// Spawn long running sparse trie task that forwards the final result upon completion.
    fn spawn_sparse_trie(
        thread_pool: Arc<rayon::ThreadPool>,
        config: StateRootConfig<Factory>,
        task_tx: Sender<StateRootMessage>,
    ) -> Sender<SparseTrieUpdate> {
        let (tx, rx) = mpsc::channel();
        thread_pool.spawn(move || {
            debug!(target: "engine::tree", "Starting sparse trie task");
            let result = match run_sparse_trie(config, rx) {
                Ok((state_root, trie_updates, iterations)) => {
                    StateRootMessage::RootCalculated { state_root, trie_updates, iterations }
                }
                Err(error) => StateRootMessage::RootCalculationError(error),
            };
            let _ = task_tx.send(result);
        });
        tx
    }

    /// Handles request for proof prefetch.
    fn on_prefetch_proof(&mut self, targets: MultiProofTargets) {
        extend_multi_proof_targets_ref(&mut self.fetched_proof_targets, &targets);

        self.multiproof_manager.spawn_or_queue(MultiproofInput {
            config: self.config.clone(),
            hashed_state_update: Default::default(),
            proof_targets: targets,
            proof_sequence_number: self.proof_sequencer.next_sequence(),
            state_root_message_sender: self.tx.clone(),
            source: ProofFetchSource::Prefetch,
        });
    }

    /// Handles state updates.
    ///
    /// Returns proof targets derived from the state update.
    fn on_state_update(&mut self, update: EvmState, proof_sequence_number: u64) {
        let hashed_state_update = evm_state_to_hashed_post_state(update);
        let proof_targets = get_proof_targets(&hashed_state_update, &self.fetched_proof_targets);
        extend_multi_proof_targets_ref(&mut self.fetched_proof_targets, &proof_targets);

        self.multiproof_manager.spawn_or_queue(MultiproofInput {
            config: self.config.clone(),
            hashed_state_update,
            proof_targets,
            proof_sequence_number,
            state_root_message_sender: self.tx.clone(),
            source: ProofFetchSource::StateUpdate,
        });
    }

    /// Handler for new proof calculated, aggregates all the existing sequential proofs.
    fn on_proof(
        &mut self,
        sequence_number: u64,
        update: SparseTrieUpdate,
    ) -> Option<SparseTrieUpdate> {
        let ready_proofs = self.proof_sequencer.add_proof(sequence_number, update);

        if ready_proofs.is_empty() {
            None
        } else {
            // Merge all ready proofs and state updates
            ready_proofs.into_iter().reduce(|mut acc_update, update| {
                acc_update.extend(update);
                acc_update
            })
        }
    }

    fn run(mut self, sparse_trie_tx: Sender<SparseTrieUpdate>) -> StateRootResult {
        let mut sparse_trie_tx = Some(sparse_trie_tx);

        let mut updates_received = 0;
        let mut proofs_processed = 0;

        let mut updates_finished = false;

        // Timestamp when the first state update was received
        let mut first_update_time = None;
        // Timestamp when the last state update was received
        let mut last_update_time = None;

        loop {
            trace!(target: "engine::root", "entering main channel receiving loop");
            match self.rx.recv() {
                Ok(message) => match message {
                    StateRootMessage::PrefetchProofs(targets) => {
                        trace!(target: "engine::root", "processing StateRootMessage::PrefetchProofs");
                        debug!(
                            target: "engine::root",
                            len = targets.len(),
                            "Prefetching proofs"
                        );
                        self.on_prefetch_proof(targets);
                    }
                    StateRootMessage::StateUpdate(update) => {
                        trace!(target: "engine::root", "processing StateRootMessage::StateUpdate");
                        if updates_received == 0 {
                            first_update_time = Some(Instant::now());
                            debug!(target: "engine::root", "Started state root calculation");
                        }
                        last_update_time = Some(Instant::now());

                        updates_received += 1;
                        debug!(
                            target: "engine::root",
                            len = update.len(),
                            total_updates = updates_received,
                            "Received new state update"
                        );
                        let next_sequence = self.proof_sequencer.next_sequence();
                        self.on_state_update(update, next_sequence);
                    }
                    StateRootMessage::FinishedStateUpdates => {
                        trace!(target: "engine::root", "processing StateRootMessage::FinishedStateUpdates");
                        updates_finished = true;
                    }
                    StateRootMessage::ProofCalculated(proof_calculated) => {
                        trace!(target: "engine::root", "processing StateRootMessage::ProofCalculated");
                        if proof_calculated.is_from_state_update() {
                            proofs_processed += 1;
                        }

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
                            let _ = sparse_trie_tx
                                .as_ref()
                                .expect("tx not dropped")
                                .send(combined_update);
                        }

                        let all_proofs_received = proofs_processed >= updates_received;
                        let no_pending = !self.proof_sequencer.has_pending();
                        if all_proofs_received && no_pending && updates_finished {
                            // drop the sender
                            sparse_trie_tx.take();
                            debug!(target: "engine::root", total_updates = updates_received, total_proofs = proofs_processed, "All proofs processed, ending calculation");
                        }
                    }
                    StateRootMessage::RootCalculated { state_root, trie_updates, iterations } => {
                        trace!(target: "engine::root", "processing StateRootMessage::RootCalculated");
                        let total_time =
                            first_update_time.expect("first update time should be set").elapsed();
                        let time_from_last_update =
                            last_update_time.expect("last update time should be set").elapsed();
                        debug!(
                            target: "engine::root",
                            total_updates = updates_received,
                            total_proofs = proofs_processed,
                            roots_calculated = iterations,
                            ?total_time,
                            ?time_from_last_update,
                            "All proofs processed, ending calculation"
                        );
                        return Ok(StateRootComputeOutcome {
                            state_root: (state_root, trie_updates),
                            total_time,
                            time_from_last_update,
                        });
                    }

                    StateRootMessage::ProofCalculationError(e) => {
                        return Err(ParallelStateRootError::Other(format!(
                            "could not calculate multiproof: {e:?}"
                        )))
                    }
                    StateRootMessage::RootCalculationError(e) => {
                        return Err(ParallelStateRootError::Other(format!(
                            "could not calculate state root: {e:?}"
                        )))
                    }
                },
                Err(_) => {
                    // this means our internal message channel is closed, which shouldn't happen
                    // in normal operation since we hold both ends
                    error!(
                        target: "engine::root",
                        "Internal message channel closed unexpectedly"
                    );
                    return Err(ParallelStateRootError::Other(
                        "Internal message channel closed unexpectedly".into(),
                    ));
                }
            }
        }
    }
}

/// Listen to incoming sparse trie updates and update the sparse trie.
/// Returns final state root, trie updates and the number of update iterations.
fn run_sparse_trie<Factory>(
    config: StateRootConfig<Factory>,
    update_rx: mpsc::Receiver<SparseTrieUpdate>,
) -> Result<(B256, TrieUpdates, u64), ParallelStateRootError>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + StateCommitmentProvider,
{
    let provider_ro = config.consistent_view.provider_ro()?;
    let in_memory_trie_cursor = InMemoryTrieCursorFactory::new(
        DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
        &config.nodes_sorted,
    );
    let blinded_provider_factory = ProofBlindedProviderFactory::new(
        in_memory_trie_cursor.clone(),
        HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
            &config.state_sorted,
        ),
        config.prefix_sets.clone(),
    );

    let mut num_iterations = 0;
    let mut trie = SparseStateTrie::new(blinded_provider_factory).with_updates(true);

    while let Ok(mut update) = update_rx.recv() {
        num_iterations += 1;
        let mut num_updates = 1;
        while let Ok(next) = update_rx.try_recv() {
            update.extend(next);
            num_updates += 1;
        }

        debug!(
            target: "engine::root",
            num_updates,
            account_proofs = update.multiproof.account_subtree.len(),
            storage_proofs = update.multiproof.storages.len(),
            "Updating sparse trie"
        );

        // TODO: alexey to remind me why we are doing this
        update.targets = get_proof_targets(&update.state, &update.targets);

        let elapsed = update_sparse_trie(&mut trie, update).map_err(|e| {
            ParallelStateRootError::Other(format!("could not calculate state root: {e:?}"))
        })?;
        trace!(target: "engine::root", ?elapsed, num_iterations, "Root calculation completed");
    }

    debug!(target: "engine::root", num_iterations, "All proofs processed, ending calculation");
    let root = trie.root().expect("sparse trie should be revealed");
    let trie_updates = trie.take_trie_updates().expect("retention must be enabled");
    Ok((root, trie_updates, num_iterations))
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

/// Calculate multiproof for the targets.
#[inline]
fn calculate_multiproof<Factory>(
    thread_pool: Arc<rayon::ThreadPool>,
    config: StateRootConfig<Factory>,
    proof_targets: MultiProofTargets,
) -> ProviderResult<MultiProof>
where
    Factory:
        DatabaseProviderFactory<Provider: BlockReader> + StateCommitmentProvider + Clone + 'static,
{
    Ok(ParallelProof::new(
        config.consistent_view,
        config.nodes_sorted,
        config.state_sorted,
        config.prefix_sets,
        thread_pool,
    )
    .with_branch_node_masks(true)
    .multiproof(proof_targets)?)
}

/// Updates the sparse trie with the given proofs and state, and returns the elapsed time.
fn update_sparse_trie<BPF>(
    trie: &mut SparseStateTrie<BPF>,
    SparseTrieUpdate { state, targets, multiproof }: SparseTrieUpdate,
) -> SparseStateTrieResult<Duration>
where
    BPF: BlindedProviderFactory + Send + Sync,
    BPF::AccountNodeProvider: BlindedProvider + Send + Sync,
    BPF::StorageNodeProvider: BlindedProvider + Send + Sync,
{
    trace!(target: "engine::root::sparse", "Updating sparse trie");
    let started_at = Instant::now();

    // Reveal new accounts and storage slots.
    trie.reveal_multiproof(targets, multiproof)?;

    // Update storage slots with new values and calculate storage roots.
    let (tx, rx) = mpsc::channel();
    state
        .storages
        .into_iter()
        .map(|(address, storage)| (address, storage, trie.take_storage_trie(&address)))
        .par_bridge()
        .map(|(address, storage, storage_trie)| {
            trace!(target: "engine::root::sparse", ?address, "Updating storage");
            let mut storage_trie = storage_trie.ok_or(SparseTrieErrorKind::Blind)?;

            if storage.wiped {
                trace!(target: "engine::root::sparse", ?address, "Wiping storage");
                storage_trie.wipe()?;
            }
            for (slot, value) in storage.storage {
                let slot_nibbles = Nibbles::unpack(slot);
                if value.is_zero() {
                    trace!(target: "engine::root::sparse", ?address, ?slot, "Removing storage slot");
                    storage_trie.remove_leaf(&slot_nibbles)?;
                } else {
                    trace!(target: "engine::root::sparse", ?address, ?slot, "Updating storage slot");
                    storage_trie
                        .update_leaf(slot_nibbles, alloy_rlp::encode_fixed_size(&value).to_vec())?;
                }
            }

            storage_trie.root();

            SparseStateTrieResult::Ok((address, storage_trie))
        })
        .for_each_init(|| tx.clone(), |tx, result| {
            tx.send(result).unwrap()
        });
    drop(tx);
    for result in rx {
        let (address, storage_trie) = result?;
        trie.insert_storage_trie(address, storage_trie);
    }

    // Update accounts with new values
    for (address, account) in state.accounts {
        trace!(target: "engine::root::sparse", ?address, "Updating account");
        trie.update_account(address, account.unwrap_or_default())?;
    }

    trie.calculate_below_level(SPARSE_TRIE_INCREMENTAL_LEVEL);
    let elapsed = started_at.elapsed();

    Ok(elapsed)
}

fn extend_multi_proof_targets(targets: &mut MultiProofTargets, other: MultiProofTargets) {
    for (address, slots) in other {
        targets.entry(address).or_default().extend(slots);
    }
}

fn extend_multi_proof_targets_ref(targets: &mut MultiProofTargets, other: &MultiProofTargets) {
    for (address, slots) in other {
        targets.entry(*address).or_default().extend(slots);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::{Account as RethAccount, StorageEntry};
    use reth_provider::{
        providers::ConsistentDbView, test_utils::create_test_provider_factory, HashingWriter,
    };
    use reth_testing_utils::generators::{self, Rng};
    use reth_trie::{test_utils::state_root, TrieInput};
    use revm_primitives::{
        Account as RevmAccount, AccountInfo, AccountStatus, Address, EvmState, EvmStorageSlot,
        HashMap, B256, KECCAK_EMPTY, U256,
    };
    use std::sync::Arc;

    fn convert_revm_to_reth_account(revm_account: &RevmAccount) -> RethAccount {
        RethAccount {
            balance: revm_account.info.balance,
            nonce: revm_account.info.nonce,
            bytecode_hash: if revm_account.info.code_hash == KECCAK_EMPTY {
                None
            } else {
                Some(revm_account.info.code_hash)
            },
        }
    }

    fn create_mock_state_updates(num_accounts: usize, updates_per_account: usize) -> Vec<EvmState> {
        let mut rng = generators::rng();
        let all_addresses: Vec<Address> = (0..num_accounts).map(|_| rng.gen()).collect();
        let mut updates = Vec::new();

        for _ in 0..updates_per_account {
            let num_accounts_in_update = rng.gen_range(1..=num_accounts);
            let mut state_update = EvmState::default();

            let selected_addresses = &all_addresses[0..num_accounts_in_update];

            for &address in selected_addresses {
                let mut storage = HashMap::default();
                if rng.gen_bool(0.7) {
                    for _ in 0..rng.gen_range(1..10) {
                        let slot = U256::from(rng.gen::<u64>());
                        storage.insert(
                            slot,
                            EvmStorageSlot::new_changed(U256::ZERO, U256::from(rng.gen::<u64>())),
                        );
                    }
                }

                let account = RevmAccount {
                    info: AccountInfo {
                        balance: U256::from(rng.gen::<u64>()),
                        nonce: rng.gen::<u64>(),
                        code_hash: KECCAK_EMPTY,
                        code: Some(Default::default()),
                    },
                    storage,
                    status: AccountStatus::Touched,
                };

                state_update.insert(address, account);
            }

            updates.push(state_update);
        }

        updates
    }

    #[test]
    fn test_state_root_task() {
        reth_tracing::init_test_tracing();

        let factory = create_test_provider_factory();

        let state_updates = create_mock_state_updates(10, 10);
        let mut hashed_state = HashedPostState::default();
        let mut accumulated_state: HashMap<Address, (RethAccount, HashMap<B256, U256>)> =
            HashMap::default();

        {
            let provider_rw = factory.provider_rw().expect("failed to get provider");

            for update in &state_updates {
                let account_updates = update.iter().map(|(address, account)| {
                    (*address, Some(convert_revm_to_reth_account(account)))
                });
                provider_rw
                    .insert_account_for_hashing(account_updates)
                    .expect("failed to insert accounts");

                let storage_updates = update.iter().map(|(address, account)| {
                    let storage_entries = account.storage.iter().map(|(slot, value)| {
                        StorageEntry { key: B256::from(*slot), value: value.present_value }
                    });
                    (*address, storage_entries)
                });
                provider_rw
                    .insert_storage_for_hashing(storage_updates)
                    .expect("failed to insert storage");
            }
            provider_rw.commit().expect("failed to commit changes");
        }

        for update in &state_updates {
            hashed_state.extend(evm_state_to_hashed_post_state(update.clone()));

            for (address, account) in update {
                let storage: HashMap<B256, U256> = account
                    .storage
                    .iter()
                    .map(|(k, v)| (B256::from(*k), v.present_value))
                    .collect();

                let entry = accumulated_state.entry(*address).or_default();
                entry.0 = convert_revm_to_reth_account(account);
                entry.1.extend(storage);
            }
        }

        let input = TrieInput::from_state(hashed_state);
        let nodes_sorted = Arc::new(input.nodes.clone().into_sorted());
        let state_sorted = Arc::new(input.state.clone().into_sorted());
        let config = StateRootConfig {
            consistent_view: ConsistentDbView::new(factory, None),
            nodes_sorted,
            state_sorted,
            prefix_sets: Arc::new(input.prefix_sets),
        };

        let num_threads = thread_pool_size();

        let state_root_task_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .thread_name(|i| format!("proof-worker-{}", i))
            .build()
            .expect("Failed to create proof worker thread pool");

        let task = StateRootTask::new(config, Arc::new(state_root_task_pool));
        let mut state_hook = task.state_hook();
        let handle = task.spawn();

        for update in state_updates {
            state_hook.on_state(&update);
        }
        drop(state_hook);

        let (root_from_task, _) = handle.wait_for_result().expect("task failed").state_root;
        let root_from_base = state_root(accumulated_state);

        assert_eq!(
            root_from_task, root_from_base,
            "State root mismatch: task={root_from_task:?}, base={root_from_base:?}"
        );
    }

    #[test]
    fn test_add_proof_in_sequence() {
        let mut sequencer = ProofSequencer::new();
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
        let mut sequencer = ProofSequencer::new();
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
        let mut sequencer = ProofSequencer::new();
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
        let mut sequencer = ProofSequencer::new();
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
        let mut sequencer = ProofSequencer::new();
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
}
