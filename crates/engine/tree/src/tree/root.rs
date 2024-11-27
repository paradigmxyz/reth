//! State root task related functionality.

use alloy_primitives::map::{DefaultHashBuilder, FbHashMap, FbHashSet, HashMap, HashSet};
use alloy_rlp::{BufMut, Encodable};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use reth_errors::ProviderResult;
use reth_execution_errors::TrieWitnessError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory,
};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory,
    proof::Proof,
    trie_cursor::InMemoryTrieCursorFactory,
    updates::{TrieUpdates, TrieUpdatesSorted},
    witness::{next_root_from_proofs, target_nodes},
    HashedPostState, HashedPostStateSorted, HashedStorage, MultiProof, Nibbles, TrieAccount,
    TrieInput, EMPTY_ROOT_HASH,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseProof, DatabaseTrieCursorFactory};
use reth_trie_parallel::root::ParallelStateRootError;
use reth_trie_sparse::{SparseStateTrie, SparseStateTrieResult};
use revm_primitives::{keccak256, EvmState, B256};
use std::{
    collections::BTreeMap,
    sync::{
        mpsc::{self, Receiver, RecvError, Sender},
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

/// Wrapper for std channel receiver to maintain compatibility with `UnboundedReceiverStream`
#[derive(Debug)]
pub(crate) struct StdReceiverStream {
    rx: Receiver<EvmState>,
}

#[allow(dead_code)]
impl StdReceiverStream {
    pub(crate) const fn new(rx: Receiver<EvmState>) -> Self {
        Self { rx }
    }

    pub(crate) fn recv(&self) -> Result<EvmState, RecvError> {
        self.rx.recv()
    }
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
        /// The index of this proof in the sequence of state updates
        sequence_number: u64,
    },
    /// State root calculation completed
    RootCalculated {
        /// The calculated state root
        root: B256,
        /// The trie updates produced during calculation
        updates: TrieUpdates,
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
    /// Buffer for out-of-order proofs
    pending_proofs: BTreeMap<u64, MultiProof>,
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

    /// Adds a proof and returns all sequential proofs if we have a continuous sequence
    pub(crate) fn add_proof(&mut self, sequence: u64, proof: MultiProof) -> Vec<MultiProof> {
        if sequence >= self.next_to_deliver {
            self.pending_proofs.insert(sequence, proof);
        }

        // return early if we don't have the next expected proof
        if !self.pending_proofs.contains_key(&self.next_to_deliver) {
            return Vec::new()
        }

        let mut consecutive_proofs = Vec::with_capacity(self.pending_proofs.len());
        let mut current_sequence = self.next_to_deliver;

        // keep collecting proofs as long as we have consecutive sequence numbers
        while let Some(proof) = self.pending_proofs.remove(&current_sequence) {
            consecutive_proofs.push(proof);
            current_sequence += 1;

            // if we don't have the next number, stop collecting
            if !self.pending_proofs.contains_key(&current_sequence) {
                break;
            }
        }

        if !consecutive_proofs.is_empty() {
            self.next_to_deliver += consecutive_proofs.len() as u64;
        }

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
    /// Receiver for state root related messages
    rx: Receiver<StateRootMessage>,
    /// Sender for state root related messages
    tx: Sender<StateRootMessage>,
    /// Task configuration
    config: StateRootConfig<Factory>,
    /// Current state
    state: HashedPostState,
    /// Proof sequencing handler
    proof_sequencer: ProofSequencer,
    /// Whether we're currently calculating a root
    calculating_root: bool,
}

#[allow(dead_code)]
impl<Factory> StateRootTask<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + Send + Sync + 'static,
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
            state: Default::default(),
            proof_sequencer: ProofSequencer::new(),
            calculating_root: false,
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
    fn on_state_update(
        view: ConsistentDbView<Factory>,
        input: Arc<TrieInput>,
        update: EvmState,
        state: &mut HashedPostState,
        proof_sequence_number: u64,
        state_root_message_sender: Sender<StateRootMessage>,
    ) {
        let mut hashed_state_update = HashedPostState::default();
        for (address, account) in update {
            if account.is_touched() {
                let hashed_address = keccak256(address);

                let destroyed = account.is_selfdestructed();
                hashed_state_update.accounts.insert(
                    hashed_address,
                    if destroyed || account.is_empty() { None } else { Some(account.info.into()) },
                );

                if destroyed || !account.storage.is_empty() {
                    let storage = account.storage.into_iter().filter_map(|(slot, value)| {
                        value
                            .is_changed()
                            .then(|| (keccak256(B256::from(slot)), value.present_value))
                    });
                    hashed_state_update
                        .storages
                        .insert(hashed_address, HashedStorage::from_iter(destroyed, storage));
                }
            }
        }

        // Dispatch proof gathering for this state update
        let targets = hashed_state_update
            .accounts
            .keys()
            .filter(|hashed_address| {
                !state.accounts.contains_key(*hashed_address) &&
                    !state.storages.contains_key(*hashed_address)
            })
            .map(|hashed_address| (*hashed_address, HashSet::default()))
            .chain(hashed_state_update.storages.iter().map(|(hashed_address, storage)| {
                (*hashed_address, storage.storage.keys().copied().collect())
            }))
            .collect::<HashMap<_, _>>();

        rayon::spawn(move || {
            let provider = match view.provider_ro() {
                Ok(provider) => provider,
                Err(error) => {
                    error!(target: "engine::root", ?error, "Could not get provider");
                    return;
                }
            };

            // TODO: replace with parallel proof
            let result =
                Proof::overlay_multiproof(provider.tx_ref(), input.as_ref().clone(), targets);
            match result {
                Ok(proof) => {
                    let _ = state_root_message_sender.send(StateRootMessage::ProofCalculated {
                        proof,
                        sequence_number: proof_sequence_number,
                    });
                }
                Err(e) => {
                    error!(target: "engine::root", error = ?e, "Could not calculate multiproof");
                }
            }
        });

        state.extend(hashed_state_update);
    }

    /// Handler for new proof calculated, aggregates all the existing sequential proofs.
    fn on_proof(&mut self, proof: MultiProof, sequence_number: u64) -> Option<MultiProof> {
        let ready_proofs = self.proof_sequencer.add_proof(sequence_number, proof);

        if ready_proofs.is_empty() {
            None
        } else {
            // combine all ready proofs into one
            ready_proofs.into_iter().reduce(|mut acc, proof| {
                acc.extend(proof);
                acc
            })
        }
    }

    /// Spawns root calculation with the current state and proofs
    fn spawn_root_calculation(&mut self, multiproof: MultiProof) {
        if self.calculating_root {
            return;
        }
        self.calculating_root = true;

        trace!(
            target: "engine::root",
            account_proofs = multiproof.account_subtree.len(),
            storage_proofs = multiproof.storages.len(),
            "Spawning root calculation"
        );

        let tx = self.tx.clone();
        let view = self.config.consistent_view.clone();
        let input = self.config.input.clone();
        let state = self.state.clone();

        rayon::spawn(move || {
            let result = calculate_state_root_from_proofs(
                view,
                &input.nodes.clone().into_sorted(),
                &input.state.clone().into_sorted(),
                multiproof,
                state,
            );
            match result {
                Ok((root, updates, elapsed)) => {
                    trace!(
                        target: "engine::root",
                        %root,
                        ?elapsed,
                        "Root calculation completed, sending result"
                    );
                    let _ = tx.send(StateRootMessage::RootCalculated { root, updates, elapsed });
                }
                Err(e) => {
                    error!(target: "engine::root", error = ?e, "Could not calculate state root");
                }
            }
        });
    }

    fn run(mut self) -> StateRootResult {
        let mut current_multiproof = MultiProof::default();
        let mut trie_updates = TrieUpdates::default();
        let mut current_root: B256;
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
                            &mut self.state,
                            self.proof_sequencer.next_sequence(),
                            self.tx.clone(),
                        );
                    }
                    StateRootMessage::ProofCalculated { proof, sequence_number } => {
                        proofs_processed += 1;
                        trace!(
                            target: "engine::root",
                            sequence = sequence_number,
                            total_proofs = proofs_processed,
                            "Processing calculated proof"
                        );

                        if let Some(combined_proof) = self.on_proof(proof, sequence_number) {
                            if self.calculating_root {
                                current_multiproof.extend(combined_proof);
                            } else {
                                self.spawn_root_calculation(combined_proof);
                            }
                        }
                    }
                    StateRootMessage::RootCalculated { root, updates, elapsed } => {
                        roots_calculated += 1;
                        trace!(
                            target: "engine::root",
                            %root,
                            ?elapsed,
                            roots_calculated,
                            proofs = proofs_processed,
                            updates = updates_received,
                            "Computed intermediate root"
                        );
                        current_root = root;
                        trie_updates.extend(updates);
                        self.calculating_root = false;

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
                            self.spawn_root_calculation(std::mem::take(&mut current_multiproof));
                        } else if all_proofs_received && no_pending {
                            debug!(
                                target: "engine::root",
                                total_updates = updates_received,
                                total_proofs = proofs_processed,
                                roots_calculated,
                                "All proofs processed, ending calculation"
                            );
                            return Ok((current_root, trie_updates));
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

/// Calculate state root from proofs.
pub fn calculate_state_root_from_proofs<Factory>(
    view: ConsistentDbView<Factory>,
    input_nodes_sorted: &TrieUpdatesSorted,
    input_state_sorted: &HashedPostStateSorted,
    multiproof: MultiProof,
    state: HashedPostState,
) -> ProviderResult<(B256, TrieUpdates, Duration)>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone,
{
    let started_at = Instant::now();

    let proof_targets: HashMap<B256, HashSet<B256>> = state
        .accounts
        .keys()
        .map(|hashed_address| (*hashed_address, HashSet::default()))
        .chain(state.storages.iter().map(|(hashed_address, storage)| {
            (*hashed_address, storage.storage.keys().copied().collect())
        }))
        .collect();

    let account_trie_nodes = proof_targets
        .into_par_iter()
        .map_init(
            || view.provider_ro().unwrap(),
            |provider_ro, (hashed_address, hashed_slots)| {
                // Gather and record storage trie nodes for this account.
                let mut storage_trie_nodes = BTreeMap::default();
                let storage = state.storages.get(&hashed_address);
                for hashed_slot in hashed_slots {
                    let slot_key = Nibbles::unpack(hashed_slot);
                    let slot_value = storage
                        .and_then(|s| s.storage.get(&hashed_slot))
                        .filter(|v| !v.is_zero())
                        .map(|v| alloy_rlp::encode_fixed_size(v).to_vec());
                    let proof = multiproof
                        .storages
                        .get(&hashed_address)
                        .map(|proof| {
                            proof
                                .subtree
                                .iter()
                                .filter(|e| slot_key.starts_with(e.0))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    storage_trie_nodes.extend(target_nodes(
                        slot_key.clone(),
                        slot_value,
                        None,
                        proof,
                    )?);
                }

                let storage_root = next_root_from_proofs(storage_trie_nodes, |key: Nibbles| {
                    // Right pad the target with 0s.
                    let mut padded_key = key.pack();
                    padded_key.resize(32, 0);
                    let mut targets = HashMap::with_hasher(DefaultHashBuilder::default());
                    let mut slots = HashSet::with_hasher(DefaultHashBuilder::default());
                    slots.insert(B256::from_slice(&padded_key));
                    targets.insert(hashed_address, slots);
                    let proof = Proof::new(
                        InMemoryTrieCursorFactory::new(
                            DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
                            input_nodes_sorted,
                        ),
                        HashedPostStateCursorFactory::new(
                            DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
                            input_state_sorted,
                        ),
                    )
                    .multiproof(targets)
                    .unwrap();

                    // The subtree only contains the proof for a single target.
                    let node = proof
                        .storages
                        .get(&hashed_address)
                        .and_then(|storage_multiproof| storage_multiproof.subtree.get(&key))
                        .cloned()
                        .ok_or(TrieWitnessError::MissingTargetNode(key))?;
                    Ok(node)
                })?;

                // Gather and record account trie nodes.
                let account = state
                    .accounts
                    .get(&hashed_address)
                    .ok_or(TrieWitnessError::MissingAccount(hashed_address))?;
                let value = (account.is_some() || storage_root != EMPTY_ROOT_HASH).then(|| {
                    let mut encoded = Vec::with_capacity(128);
                    TrieAccount::from((account.unwrap_or_default(), storage_root))
                        .encode(&mut encoded as &mut dyn BufMut);
                    encoded
                });
                let key = Nibbles::unpack(hashed_address);
                let proof = multiproof.account_subtree.iter().filter(|e| key.starts_with(e.0));
                target_nodes(key.clone(), value, None, proof)
            },
        )
        .try_reduce(BTreeMap::new, |mut acc, map| {
            acc.extend(map.into_iter());
            Ok(acc)
        })?;

    let provider_ro = view.provider_ro()?;

    let state_root = next_root_from_proofs(account_trie_nodes, |key: Nibbles| {
        // Right pad the target with 0s.
        let mut padded_key = key.pack();
        padded_key.resize(32, 0);
        let mut targets = HashMap::with_hasher(DefaultHashBuilder::default());
        targets.insert(
            B256::from_slice(&padded_key),
            HashSet::with_hasher(DefaultHashBuilder::default()),
        );
        let proof = Proof::new(
            InMemoryTrieCursorFactory::new(
                DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
                input_nodes_sorted,
            ),
            HashedPostStateCursorFactory::new(
                DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
                input_state_sorted,
            ),
        )
        .multiproof(targets)
        .unwrap();

        // The subtree only contains the proof for a single target.
        let node = proof
            .account_subtree
            .get(&key)
            .cloned()
            .ok_or(TrieWitnessError::MissingTargetNode(key))?;
        Ok(node)
    })?;

    Ok((state_root, Default::default(), started_at.elapsed()))
}

/// Updates the sparse trie with the given proofs and state, and returns the updated trie and the
/// time it took.
#[allow(dead_code)]
fn update_sparse_trie(
    mut trie: Box<SparseStateTrie>,
    multiproof: MultiProof,
    targets: FbHashMap<32, FbHashSet<32>>,
    state: HashedPostState,
) -> SparseStateTrieResult<(Box<SparseStateTrie>, Duration)> {
    let started_at = Instant::now();

    // Reveal new accounts and storage slots.
    for (address, slots) in targets {
        let path = Nibbles::unpack(address);
        trie.reveal_account(address, multiproof.account_proof_nodes(&path))?;

        let storage_proofs = multiproof.storage_proof_nodes(address, slots);

        for (slot, proof) in storage_proofs {
            trie.reveal_storage_slot(address, slot, proof)?;
        }
    }

    // Update storage slots with new values and calculate storage roots.
    let mut storage_roots = FbHashMap::default();
    for (address, storage) in state.storages {
        if storage.wiped {
            trie.wipe_storage(address)?;
            storage_roots.insert(address, EMPTY_ROOT_HASH);
        }

        for (slot, value) in storage.storage {
            let slot_path = Nibbles::unpack(slot);
            trie.update_storage_leaf(
                address,
                slot_path,
                alloy_rlp::encode_fixed_size(&value).to_vec(),
            )?;
        }

        storage_roots.insert(address, trie.storage_root(address).unwrap());
    }

    // Update accounts with new values and include updated storage roots
    for (address, account) in state.accounts {
        let path = Nibbles::unpack(address);

        if let Some(account) = account {
            let storage_root = storage_roots
                .remove(&address)
                .map(Some)
                .unwrap_or_else(|| trie.storage_root(address))
                .unwrap_or(EMPTY_ROOT_HASH);

            let mut encoded = Vec::with_capacity(128);
            TrieAccount::from((account, storage_root)).encode(&mut encoded as &mut dyn BufMut);
            trie.update_account_leaf(path, encoded)?;
        } else {
            trie.remove_account_leaf(&path)?;
        }
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

        let ready = sequencer.add_proof(0, proof1);
        assert_eq!(ready.len(), 1);
        assert!(!sequencer.has_pending());

        let ready = sequencer.add_proof(1, proof2);
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

        let ready = sequencer.add_proof(2, proof3);
        assert_eq!(ready.len(), 0);
        assert!(sequencer.has_pending());

        let ready = sequencer.add_proof(0, proof1);
        assert_eq!(ready.len(), 1);
        assert!(sequencer.has_pending());

        let ready = sequencer.add_proof(1, proof2);
        assert_eq!(ready.len(), 2);
        assert!(!sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_with_gaps() {
        let mut sequencer = ProofSequencer::new();
        let proof1 = MultiProof::default();
        let proof3 = MultiProof::default();
        sequencer.next_sequence = 3;

        let ready = sequencer.add_proof(0, proof1);
        assert_eq!(ready.len(), 1);

        let ready = sequencer.add_proof(2, proof3);
        assert_eq!(ready.len(), 0);
        assert!(sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_duplicate_sequence() {
        let mut sequencer = ProofSequencer::new();
        let proof1 = MultiProof::default();
        let proof2 = MultiProof::default();

        let ready = sequencer.add_proof(0, proof1);
        assert_eq!(ready.len(), 1);

        let ready = sequencer.add_proof(0, proof2);
        assert_eq!(ready.len(), 0);
        assert!(!sequencer.has_pending());
    }

    #[test]
    fn test_add_proof_batch_processing() {
        let mut sequencer = ProofSequencer::new();
        let proofs: Vec<_> = (0..5).map(|_| MultiProof::default()).collect();
        sequencer.next_sequence = 5;

        sequencer.add_proof(4, proofs[4].clone());
        sequencer.add_proof(2, proofs[2].clone());
        sequencer.add_proof(1, proofs[1].clone());
        sequencer.add_proof(3, proofs[3].clone());

        let ready = sequencer.add_proof(0, proofs[0].clone());
        assert_eq!(ready.len(), 5);
        assert!(!sequencer.has_pending());
    }
}
