//! State root task related functionality.

use alloy_primitives::map::{HashMap, HashSet};
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory,
    StateCommitmentProvider,
};
use reth_trie::{
    proof::Proof, updates::TrieUpdates, HashedPostState, HashedStorage, MultiProof, Nibbles,
    TrieInput,
};
use reth_trie_db::DatabaseProof;
use reth_trie_parallel::root::ParallelStateRootError;
use reth_trie_sparse::{
    errors::{SparseStateTrieResult, SparseTrieError},
    SparseStateTrie,
};
use revm_primitives::{keccak256, EvmState, B256};
use std::{
    collections::BTreeMap,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    time::{Duration, Instant},
};
use tracing::{debug, error, trace};

/// The level below which the sparse trie hashes are calculated in [`update_sparse_trie`].
const SPARSE_TRIE_INCREMENTAL_LEVEL: usize = 2;

/// Result of the state root calculation
pub(crate) type StateRootResult = Result<(B256, TrieUpdates), ParallelStateRootError>;

/// Handle to a spawned state root task.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct StateRootHandle {
    /// Channel for receiving the final result.
    rx: mpsc::Receiver<StateRootResult>,
}

#[allow(dead_code)]
impl StateRootHandle {
    /// Creates a new handle from a receiver.
    pub(crate) const fn new(rx: mpsc::Receiver<StateRootResult>) -> Self {
        Self { rx }
    }

    /// Waits for the state root calculation to complete.
    pub(crate) fn wait_for_result(self) -> StateRootResult {
        self.rx.recv().expect("state root task was dropped without sending result")
    }
}

/// Common configuration for state root tasks
#[derive(Debug)]
pub(crate) struct StateRootConfig<Factory> {
    /// View over the state in the database.
    pub consistent_view: ConsistentDbView<Factory>,
    /// Latest trie input.
    pub input: Arc<TrieInput>,
}

/// Messages used internally by the state root task
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum StateRootMessage {
    /// New state update from transaction execution
    StateUpdate(EvmState),
    /// Proof calculation completed for a specific state update
    ProofCalculated {
        /// The calculated proof
        proof: MultiProof,
        /// The state update that was used to calculate the proof
        state_update: HashedPostState,
        /// The index of this proof in the sequence of state updates
        sequence_number: u64,
    },
    /// State root calculation completed
    RootCalculated {
        /// The updated sparse trie
        trie: Box<SparseStateTrie>,
        /// Time taken to calculate the root
        elapsed: Duration,
    },
}

/// Handle to track proof calculation ordering
#[derive(Debug, Default)]
pub(crate) struct ProofSequencer {
    /// The next proof sequence number to be produced.
    next_sequence: u64,
    /// The next sequence number expected to be delivered.
    next_to_deliver: u64,
    /// Buffer for out-of-order proofs and corresponding state updates
    pending_proofs: BTreeMap<u64, (MultiProof, HashedPostState)>,
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
        proof: MultiProof,
        state_update: HashedPostState,
    ) -> Vec<(MultiProof, HashedPostState)> {
        if sequence >= self.next_to_deliver {
            self.pending_proofs.insert(sequence, (proof, state_update));
        }

        // return early if we don't have the next expected proof
        if !self.pending_proofs.contains_key(&self.next_to_deliver) {
            return Vec::new()
        }

        let mut consecutive_proofs = Vec::with_capacity(self.pending_proofs.len());
        let mut current_sequence = self.next_to_deliver;

        // keep collecting proofs and state updates as long as we have consecutive sequence numbers
        while let Some((proof, state_update)) = self.pending_proofs.remove(&current_sequence) {
            consecutive_proofs.push((proof, state_update));
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

/// Standalone task that receives a transaction state stream and updates relevant
/// data structures to calculate state root.
///
/// It is responsible of  initializing a blinded sparse trie and subscribe to
/// transaction state stream. As it receives transaction execution results, it
/// fetches the proofs for relevant accounts from the database and reveal them
/// to the tree.
/// Then it updates relevant leaves according to the result of the transaction.
#[derive(Debug)]
pub(crate) struct StateRootTask<Factory> {
    /// Task configuration.
    config: StateRootConfig<Factory>,
    /// Receiver for state root related messages.
    rx: Receiver<StateRootMessage>,
    /// Sender for state root related messages.
    tx: Sender<StateRootMessage>,
    /// Proof targets that have been already fetched.
    fetched_proof_targets: HashMap<B256, HashSet<B256>>,
    /// Proof sequencing handler.
    proof_sequencer: ProofSequencer,
    /// The sparse trie used for the state root calculation. If [`None`], then update is in
    /// progress.
    sparse_trie: Option<Box<SparseStateTrie>>,
}

#[allow(dead_code)]
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
    pub(crate) fn new(
        config: StateRootConfig<Factory>,
        tx: Sender<StateRootMessage>,
        rx: Receiver<StateRootMessage>,
    ) -> Self {
        Self {
            config,
            rx,
            tx,
            fetched_proof_targets: Default::default(),
            proof_sequencer: ProofSequencer::new(),
            sparse_trie: Some(Box::new(SparseStateTrie::default().with_updates(true))),
        }
    }

    /// Spawns the state root task and returns a handle to await its result.
    pub(crate) fn spawn(self) -> StateRootHandle {
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("State Root Task".to_string())
            .spawn(move || {
                debug!(target: "engine::tree", "Starting state root task");
                let result = self.run();
                let _ = tx.send(result);
            })
            .expect("failed to spawn state root thread");

        StateRootHandle::new(rx)
    }

    /// Handles state updates.
    ///
    /// Returns proof targets derived from the state update.
    fn on_state_update(
        view: ConsistentDbView<Factory>,
        input: Arc<TrieInput>,
        update: EvmState,
        fetched_proof_targets: &mut HashMap<B256, HashSet<B256>>,
        proof_sequence_number: u64,
        state_root_message_sender: Sender<StateRootMessage>,
    ) {
        let mut hashed_state_update = HashedPostState::default();
        for (address, account) in update {
            if account.is_touched() {
                let hashed_address = keccak256(address);
                trace!(target: "engine::root", ?address, ?hashed_address, "Adding account to state update");

                let destroyed = account.is_selfdestructed();
                let info = if account.is_empty() { None } else { Some(account.info.into()) };
                hashed_state_update.accounts.insert(hashed_address, info);

                let mut changed_storage_iter = account
                    .storage
                    .into_iter()
                    .filter_map(|(slot, value)| {
                        value
                            .is_changed()
                            .then(|| (keccak256(B256::from(slot)), value.present_value))
                    })
                    .peekable();
                if destroyed || changed_storage_iter.peek().is_some() {
                    hashed_state_update.storages.insert(
                        hashed_address,
                        HashedStorage::from_iter(destroyed, changed_storage_iter),
                    );
                }
            }
        }

        let proof_targets = get_proof_targets(&hashed_state_update, fetched_proof_targets);
        for (address, slots) in &proof_targets {
            fetched_proof_targets.entry(*address).or_default().extend(slots)
        }

        // Dispatch proof gathering for this state update
        rayon::spawn(move || {
            let provider = match view.provider_ro() {
                Ok(provider) => provider,
                Err(error) => {
                    error!(target: "engine::root", ?error, "Could not get provider");
                    return;
                }
            };

            // TODO: replace with parallel proof
            let result = Proof::overlay_multiproof(
                provider.tx_ref(),
                // TODO(alexey): this clone can be expensive, we should avoid it
                input.as_ref().clone(),
                proof_targets,
            );
            match result {
                Ok(proof) => {
                    let _ = state_root_message_sender.send(StateRootMessage::ProofCalculated {
                        proof,
                        state_update: hashed_state_update,
                        sequence_number: proof_sequence_number,
                    });
                }
                Err(e) => {
                    error!(target: "engine::root", error = ?e, "Could not calculate multiproof");
                }
            }
        });
    }

    /// Handler for new proof calculated, aggregates all the existing sequential proofs.
    fn on_proof(
        &mut self,
        sequence_number: u64,
        proof: MultiProof,
        state_update: HashedPostState,
    ) -> Option<(MultiProof, HashedPostState)> {
        let ready_proofs = self.proof_sequencer.add_proof(sequence_number, proof, state_update);

        if ready_proofs.is_empty() {
            None
        } else {
            // Merge all ready proofs and state updates
            ready_proofs.into_iter().reduce(|mut acc, (proof, state_update)| {
                acc.0.extend(proof);
                acc.1.extend(state_update);
                acc
            })
        }
    }

    /// Spawns root calculation with the current state and proofs.
    fn spawn_root_calculation(&mut self, state: HashedPostState, multiproof: MultiProof) {
        let Some(trie) = self.sparse_trie.take() else { return };

        trace!(
            target: "engine::root",
            account_proofs = multiproof.account_subtree.len(),
            storage_proofs = multiproof.storages.len(),
            "Spawning root calculation"
        );

        // TODO(alexey): store proof targets in `ProofSequecner` to avoid recomputing them
        let targets = get_proof_targets(&state, &HashMap::default());

        let tx = self.tx.clone();
        rayon::spawn(move || {
            let result = update_sparse_trie(trie, multiproof, targets, state);
            match result {
                Ok((trie, elapsed)) => {
                    trace!(
                        target: "engine::root",
                        ?elapsed,
                        "Root calculation completed, sending result"
                    );
                    let _ = tx.send(StateRootMessage::RootCalculated { trie, elapsed });
                }
                Err(e) => {
                    error!(target: "engine::root", error = ?e, "Could not calculate state root");
                }
            }
        });
    }

    fn run(mut self) -> StateRootResult {
        let mut current_state_update = HashedPostState::default();
        let mut current_multiproof = MultiProof::default();
        let mut updates_received = 0;
        let mut proofs_processed = 0;
        let mut roots_calculated = 0;

        loop {
            match self.rx.recv() {
                Ok(message) => match message {
                    StateRootMessage::StateUpdate(update) => {
                        updates_received += 1;
                        trace!(
                            target: "engine::root",
                            len = update.len(),
                            total_updates = updates_received,
                            "Received new state update"
                        );
                        Self::on_state_update(
                            self.config.consistent_view.clone(),
                            self.config.input.clone(),
                            update,
                            &mut self.fetched_proof_targets,
                            self.proof_sequencer.next_sequence(),
                            self.tx.clone(),
                        );
                    }
                    StateRootMessage::ProofCalculated { proof, state_update, sequence_number } => {
                        proofs_processed += 1;
                        trace!(
                            target: "engine::root",
                            sequence = sequence_number,
                            total_proofs = proofs_processed,
                            "Processing calculated proof"
                        );

                        trace!(target: "engine::root", ?proof, "Proof calculated");

                        if let Some((combined_proof, combined_state_update)) =
                            self.on_proof(sequence_number, proof, state_update)
                        {
                            if self.sparse_trie.is_none() {
                                current_multiproof.extend(combined_proof);
                                current_state_update.extend(combined_state_update);
                            } else {
                                self.spawn_root_calculation(combined_state_update, combined_proof);
                            }
                        }
                    }
                    StateRootMessage::RootCalculated { trie, elapsed } => {
                        roots_calculated += 1;
                        trace!(
                            target: "engine::root",
                            ?elapsed,
                            roots_calculated,
                            proofs = proofs_processed,
                            updates = updates_received,
                            "Computed intermediate root"
                        );
                        self.sparse_trie = Some(trie);

                        let has_new_proofs = !current_multiproof.account_subtree.is_empty() ||
                            !current_multiproof.storages.is_empty();
                        let all_proofs_received = proofs_processed >= updates_received;
                        let no_pending = !self.proof_sequencer.has_pending();

                        trace!(
                            target: "engine::root",
                            has_new_proofs,
                            all_proofs_received,
                            no_pending,
                            "State check"
                        );

                        // only spawn new calculation if we have accumulated new proofs
                        if has_new_proofs {
                            trace!(
                                target: "engine::root",
                                account_proofs = current_multiproof.account_subtree.len(),
                                storage_proofs = current_multiproof.storages.len(),
                                "Spawning subsequent root calculation"
                            );
                            self.spawn_root_calculation(
                                std::mem::take(&mut current_state_update),
                                std::mem::take(&mut current_multiproof),
                            );
                        } else if all_proofs_received && no_pending {
                            debug!(
                                target: "engine::root",
                                total_updates = updates_received,
                                total_proofs = proofs_processed,
                                roots_calculated,
                                "All proofs processed, ending calculation"
                            );
                            let mut trie = self
                                .sparse_trie
                                .take()
                                .expect("sparse trie update should not be in progress");
                            let root = trie.root().expect("sparse trie should be revealed");
                            let trie_updates = trie
                                .take_trie_updates()
                                .expect("sparse trie should have updates retention enabled");
                            return Ok((root, trie_updates));
                        }
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

/// Returns accounts only with those storages that were not already fetched, and
/// if there are no such storages and the account itself was already fetched, the
/// account shouldn't be included.
fn get_proof_targets(
    state_update: &HashedPostState,
    fetched_proof_targets: &HashMap<B256, HashSet<B256>>,
) -> HashMap<B256, HashSet<B256>> {
    let mut targets = HashMap::default();

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

/// Updates the sparse trie with the given proofs and state, and returns the updated trie and the
/// time it took.
fn update_sparse_trie(
    mut trie: Box<SparseStateTrie>,
    multiproof: MultiProof,
    targets: HashMap<B256, HashSet<B256>>,
    state: HashedPostState,
) -> SparseStateTrieResult<(Box<SparseStateTrie>, Duration)> {
    trace!(target: "engine::root::sparse", "Updating sparse trie");
    let started_at = Instant::now();

    // Reveal new accounts and storage slots.
    trie.reveal_multiproof(targets, multiproof)?;

    // Update storage slots with new values and calculate storage roots.
    for (address, storage) in state.storages {
        trace!(target: "engine::root::sparse", ?address, "Updating storage");
        let storage_trie = trie.storage_trie_mut(&address).ok_or(SparseTrieError::Blind)?;

        if storage.wiped {
            trace!(target: "engine::root::sparse", ?address, "Wiping storage");
            storage_trie.wipe();
        }

        for (slot, value) in storage.storage {
            let slot_nibbles = Nibbles::unpack(slot);
            if value.is_zero() {
                trace!(target: "engine::root::sparse", ?address, ?slot, "Removing storage slot");

                // TODO: handle blinded node error
                storage_trie.remove_leaf(&slot_nibbles)?;
            } else {
                trace!(target: "engine::root::sparse", ?address, ?slot, "Updating storage slot");
                storage_trie
                    .update_leaf(slot_nibbles, alloy_rlp::encode_fixed_size(&value).to_vec())?;
            }
        }

        storage_trie.root();
    }

    // Update accounts with new values
    for (address, account) in state.accounts {
        trace!(target: "engine::root::sparse", ?address, "Updating account");
        trie.update_account(address, account.unwrap_or_default())?;
    }

    trie.calculate_below_level(SPARSE_TRIE_INCREMENTAL_LEVEL);
    let elapsed = started_at.elapsed();

    Ok((trie, elapsed))
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
        let (tx, rx) = std::sync::mpsc::channel();

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
            for (address, account) in update {
                let hashed_address = keccak256(*address);

                if account.is_touched() {
                    let destroyed = account.is_selfdestructed();
                    hashed_state.accounts.insert(
                        hashed_address,
                        if destroyed || account.is_empty() {
                            None
                        } else {
                            Some(account.info.clone().into())
                        },
                    );

                    if destroyed || !account.storage.is_empty() {
                        let storage = account
                            .storage
                            .iter()
                            .filter(|&(_slot, value)| (!destroyed && value.is_changed()))
                            .map(|(slot, value)| {
                                (keccak256(B256::from(*slot)), value.present_value)
                            });
                        hashed_state
                            .storages
                            .insert(hashed_address, HashedStorage::from_iter(destroyed, storage));
                    }
                }

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

        let config = StateRootConfig {
            consistent_view: ConsistentDbView::new(factory, None),
            input: Arc::new(TrieInput::from_state(hashed_state)),
        };
        let task = StateRootTask::new(config, tx.clone(), rx);
        let handle = task.spawn();

        for update in state_updates {
            tx.send(StateRootMessage::StateUpdate(update)).expect("failed to send state");
        }
        drop(tx);

        let (root_from_task, _) = handle.wait_for_result().expect("task failed");
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

        let ready = sequencer.add_proof(0, proof1, HashedPostState::default());
        assert_eq!(ready.len(), 1);
        assert!(!sequencer.has_pending());

        let ready = sequencer.add_proof(1, proof2, HashedPostState::default());
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

        let ready = sequencer.add_proof(2, proof3, HashedPostState::default());
        assert_eq!(ready.len(), 0);
        assert!(sequencer.has_pending());

        let ready = sequencer.add_proof(0, proof1, HashedPostState::default());
        assert_eq!(ready.len(), 1);
        assert!(sequencer.has_pending());

        let ready = sequencer.add_proof(1, proof2, HashedPostState::default());
        assert_eq!(ready.len(), 2);
        assert!(!sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_with_gaps() {
        let mut sequencer = ProofSequencer::new();
        let proof1 = MultiProof::default();
        let proof3 = MultiProof::default();
        sequencer.next_sequence = 3;

        let ready = sequencer.add_proof(0, proof1, HashedPostState::default());
        assert_eq!(ready.len(), 1);

        let ready = sequencer.add_proof(2, proof3, HashedPostState::default());
        assert_eq!(ready.len(), 0);
        assert!(sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_duplicate_sequence() {
        let mut sequencer = ProofSequencer::new();
        let proof1 = MultiProof::default();
        let proof2 = MultiProof::default();

        let ready = sequencer.add_proof(0, proof1, HashedPostState::default());
        assert_eq!(ready.len(), 1);

        let ready = sequencer.add_proof(0, proof2, HashedPostState::default());
        assert_eq!(ready.len(), 0);
        assert!(!sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_batch_processing() {
        let mut sequencer = ProofSequencer::new();
        let proofs: Vec<_> = (0..5).map(|_| MultiProof::default()).collect();
        sequencer.next_sequence = 5;

        sequencer.add_proof(4, proofs[4].clone(), HashedPostState::default());
        sequencer.add_proof(2, proofs[2].clone(), HashedPostState::default());
        sequencer.add_proof(1, proofs[1].clone(), HashedPostState::default());
        sequencer.add_proof(3, proofs[3].clone(), HashedPostState::default());

        let ready = sequencer.add_proof(0, proofs[0].clone(), HashedPostState::default());
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
        let fetched = HashMap::default();

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
        let fetched = HashMap::default();

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
        let mut fetched = HashMap::default();

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
        let mut fetched = HashMap::default();

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
        let fetched = HashMap::default();

        let targets = get_proof_targets(&state, &fetched);

        assert!(targets.is_empty());
    }

    #[test]
    fn test_get_proof_targets_mixed_fetched_state() {
        let mut state = HashedPostState::default();
        let mut fetched = HashMap::default();

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
        let fetched = HashMap::default();

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
