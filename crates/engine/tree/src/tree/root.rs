//! State root task related functionality.

use alloy_primitives::map::{DefaultHashBuilder, HashMap, HashSet};
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
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use reth_trie_parallel::{proof::ParallelProof, root::ParallelStateRootError};
use reth_trie_sparse::{SparseStateTrie, SparseStateTrieResult};
use revm_primitives::{keccak256, EvmState, B256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        mpsc::{self, Receiver, RecvError},
        Arc,
    },
    time::{Duration, Instant},
};
use tracing::debug;

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
#[allow(dead_code)]
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

type PendingProofs = VecDeque<
    Receiver<(
        HashMap<B256, HashSet<B256>>,
        HashedPostState,
        Result<MultiProof, ParallelStateRootError>,
    )>,
>;

type StateRootProofResult = (B256, MultiProof, TrieUpdates, Duration);
type StateRootProofReceiver = mpsc::Receiver<ProviderResult<StateRootProofResult>>;

enum StateRootTaskState {
    Idle(MultiProof, B256),
    Pending(MultiProof, StateRootProofReceiver),
}

impl StateRootTaskState {
    fn add_proofs(&mut self, proofs: MultiProof) {
        match self {
            Self::Idle(multiproof, _) | Self::Pending(multiproof, _) => {
                multiproof.extend(proofs);
            }
        }
    }
}

type SparseStateRootProofResult = (Box<SparseStateTrie>, B256, Duration);
type SparseStateRootProofReceiver =
    mpsc::Receiver<SparseStateTrieResult<SparseStateRootProofResult>>;

enum SparseStateRootTaskState {
    Idle {
        trie: Box<SparseStateTrie>,
        targets: HashMap<B256, HashSet<B256>>,
        multiproof: MultiProof,
        state_root: B256,
    },
    Pending {
        targets: HashMap<B256, HashSet<B256>>,
        multiproof: MultiProof,
        rx: SparseStateRootProofReceiver,
    },
}

impl SparseStateRootTaskState {
    fn add_proofs(&mut self, targets: HashMap<B256, HashSet<B256>>, multiproof: MultiProof) {
        match self {
            Self::Idle { targets: self_targets, multiproof: self_multiproof, .. } |
            Self::Pending { targets: self_targets, multiproof: self_multiproof, .. } => {
                for (address, slots) in targets {
                    self_targets.entry(address).or_default().extend(slots);
                }
                self_multiproof.extend(multiproof);
            }
        }
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
#[allow(dead_code)]
pub(crate) struct StateRootTask<Factory> {
    /// Incoming state updates.
    state_stream: StdReceiverStream,
    /// Task configuration.
    config: StateRootConfig<Factory>,
    /// Current state.
    state: HashedPostState,
    updated_state: HashedPostState,
    /// Channels to retrieve proof calculation results from.
    pending_proofs: PendingProofs,
    pending_calculation: bool,
    pending_sparse_calculation: bool,
}

#[allow(dead_code)]
impl<Factory> StateRootTask<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + Send + Sync + 'static,
{
    /// Creates a new `StateRootTask`.
    pub(crate) fn new(config: StateRootConfig<Factory>, state_stream: StdReceiverStream) -> Self {
        Self {
            config,
            state_stream,
            state: Default::default(),
            updated_state: Default::default(),
            pending_proofs: Default::default(),
            pending_calculation: false,
            pending_sparse_calculation: false,
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
        state: &HashedPostState,
        pending_proofs: &mut PendingProofs,
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
                        (!destroyed && value.is_changed())
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

        let (tx, rx) = mpsc::sync_channel(1);
        rayon::spawn(move || {
            let result = ParallelProof::new(view, input).multiproof(targets.clone());
            let _ = tx.send((targets.clone(), hashed_state_update, result));
        });

        pending_proofs.push_back(rx);
    }

    fn run(mut self) -> StateRootResult {
        let mut task_state = StateRootTaskState::Idle(MultiProof::default(), B256::default());
        let mut sparse_task_state = SparseStateRootTaskState::Idle {
            trie: Box::default(),
            targets: HashMap::default(),
            multiproof: MultiProof::default(),
            state_root: B256::default(),
        };
        let mut trie_updates = TrieUpdates::default();

        loop {
            // try to receive state updates without blocking
            match self.state_stream.rx.try_recv() {
                Ok(update) => {
                    debug!(target: "engine::root", len = update.len(), "Received new state update");
                    Self::on_state_update(
                        self.config.consistent_view.clone(),
                        self.config.input.clone(),
                        update,
                        &self.state,
                        &mut self.pending_proofs,
                    );
                    continue;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // no new state updates available, continue with other operations
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    // state stream closed, check if we can finish
                    if self.pending_proofs.is_empty() &&
                        !self.pending_calculation &&
                        !self.pending_sparse_calculation
                    {
                        if let (
                            StateRootTaskState::Idle(_multiproof, state_root),
                            SparseStateRootTaskState::Idle {
                                state_root: sparse_state_root, ..
                            },
                        ) = (&task_state, &sparse_task_state)
                        {
                            assert_eq!(state_root, sparse_state_root);
                            return Ok((*state_root, trie_updates));
                        }
                    }
                }
            }

            // check pending proofs
            while let Some(proof_rx) = self.pending_proofs.front() {
                match proof_rx.try_recv() {
                    Ok((targets, state, result)) => {
                        self.state.extend(state.clone());
                        self.updated_state.extend(state);

                        let multiproof = result?;
                        task_state.add_proofs(multiproof.clone());
                        sparse_task_state.add_proofs(targets, multiproof);
                        self.pending_proofs.pop_front();
                        self.pending_calculation = true;
                        self.pending_sparse_calculation = true;
                        continue;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        // this proof is not ready yet
                        break;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        // channel was closed without sending a result
                        return Err(ParallelStateRootError::Other(
                            "Proof calculation task terminated unexpectedly".into(),
                        ));
                    }
                }
            }

            // handle task state transitions
            match &mut task_state {
                StateRootTaskState::Pending(multiproof, rx) => {
                    match rx.try_recv() {
                        Ok(result) => match result {
                            Ok((state_root, mut new_multiproof, new_trie_updates, elapsed)) => {
                                debug!(target: "engine::root", %state_root, ?elapsed, "Computed intermediate root");
                                trie_updates.extend(new_trie_updates);
                                new_multiproof.extend(std::mem::take(multiproof));
                                task_state = StateRootTaskState::Idle(new_multiproof, state_root);
                            }
                            Err(e) => return Err(ParallelStateRootError::Provider(e)),
                        },
                        Err(mpsc::TryRecvError::Empty) => {
                            // root calculation not ready yet
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            return Err(ParallelStateRootError::Other(
                                "Root calculation task terminated unexpectedly".into(),
                            ));
                        }
                    }
                }
                StateRootTaskState::Idle(multiproof, _) => {
                    if self.pending_calculation {
                        debug!(target: "engine::root", accounts_len = self.state.accounts.len(), "Spawning state root calculation from proofs task");
                        let view = self.config.consistent_view.clone();
                        let input_nodes_sorted = self.config.input.nodes.clone().into_sorted();
                        let input_state_sorted = self.config.input.state.clone().into_sorted();
                        let multiproof = std::mem::take(multiproof);
                        let state = self.state.clone();
                        let (tx, rx) = mpsc::sync_channel(1);

                        self.pending_calculation = false;

                        rayon::spawn(move || {
                            let result = calculate_state_root_from_proofs(
                                view,
                                &input_nodes_sorted,
                                &input_state_sorted,
                                multiproof,
                                state,
                            );
                            let _ = tx.send(result);
                        });

                        task_state = StateRootTaskState::Pending(Default::default(), rx);
                    }
                }
            }

            match &mut sparse_task_state {
                SparseStateRootTaskState::Pending { targets, multiproof, rx } => {
                    match rx.try_recv() {
                        Ok(result) => match result {
                            Ok((trie, state_root, elapsed)) => {
                                debug!(target: "engine::root", %state_root, ?elapsed, "Computed intermediate root");
                                sparse_task_state = SparseStateRootTaskState::Idle {
                                    trie,
                                    targets: std::mem::take(targets),
                                    multiproof: std::mem::take(multiproof),
                                    state_root,
                                };
                            }
                            // TODO(alexey): proper error variant
                            Err(e) => return Err(ParallelStateRootError::Other(e.to_string())),
                        },
                        Err(mpsc::TryRecvError::Empty) => {
                            // root calculation not ready yet
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            return Err(ParallelStateRootError::Other(
                                "Root calculation task terminated unexpectedly".into(),
                            ));
                        }
                    }
                }
                SparseStateRootTaskState::Idle { trie, targets, multiproof, .. } => {
                    if self.pending_sparse_calculation {
                        debug!(target: "engine::root", accounts_len = self.state.accounts.len(), "Spawning state root calculation from proofs task");
                        let trie = std::mem::take(trie);
                        let targets = std::mem::take(targets);
                        let multiproof = std::mem::take(multiproof);
                        let updated_state = std::mem::take(&mut self.updated_state);
                        let (tx, rx) = mpsc::sync_channel(1);

                        self.pending_sparse_calculation = false;

                        rayon::spawn(move || {
                            let result = calculate_state_root_with_sparse(
                                trie,
                                multiproof,
                                targets,
                                updated_state,
                            );
                            let _ = tx.send(result);
                        });

                        sparse_task_state = SparseStateRootTaskState::Pending {
                            targets: HashMap::default(),
                            multiproof: MultiProof::default(),
                            rx,
                        };
                    }
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
) -> ProviderResult<(B256, MultiProof, TrieUpdates, Duration)>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone,
{
    let started_at = Instant::now();

    let provider_ro = view.provider_ro()?;

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
        .map(|(hashed_address, hashed_slots)| {
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
                storage_trie_nodes.extend(target_nodes(slot_key.clone(), slot_value, None, proof)?);
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
        })
        .try_reduce(BTreeMap::new, |mut acc, map| {
            acc.extend(map.into_iter());
            Ok(acc)
        })?;

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

    Ok((state_root, multiproof, Default::default(), started_at.elapsed()))
}

fn calculate_state_root_with_sparse(
    mut trie: Box<SparseStateTrie>,
    multiproof: MultiProof,
    targets: HashMap<B256, HashSet<B256>>,
    state: HashedPostState,
) -> SparseStateTrieResult<(Box<SparseStateTrie>, B256, Duration)> {
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
    let mut storage_roots = HashMap::with_capacity(state.storages.len());
    for (address, storage) in state.storages {
        if storage.wiped {
            trie.wipe_storage(address);
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
            trie.update_leaf(path, encoded)?;
        } else {
            trie.remove_leaf(&path)?;
        }
    }

    // TODO(alexey): calculate using `update_rlp_node_level` and then
    // finalize in the end
    let root = trie.root().unwrap_or(EMPTY_ROOT_HASH);
    let elapsed = started_at.elapsed();

    Ok((trie, root, elapsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{prelude::SliceRandom, Rng};
    use reth_primitives::{Account as RethAccount, StorageEntry};
    use reth_provider::{
        providers::ConsistentDbView, test_utils::create_test_provider_factory, HashingWriter,
    };
    use reth_testing_utils::generators;
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
        let mut all_addresses: Vec<Address> =
            (0..num_accounts).map(|_| rng.gen::<Address>()).collect();
        let mut updates = Vec::new();

        for _ in 0..updates_per_account {
            let num_accounts_in_update = rng.gen_range(1..=num_accounts);
            let mut state_update = EvmState::default();

            all_addresses.shuffle(&mut rng);
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
        let stream = StdReceiverStream::new(rx);

        let state_updates = create_mock_state_updates(100, 10);
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
        let task = StateRootTask::new(config, stream);
        let handle = task.spawn();

        for update in state_updates {
            tx.send(update).expect("failed to send state");
        }
        drop(tx);

        let (root_from_task, _) = handle.wait_for_result().expect("task failed");
        let root_from_base = state_root(accumulated_state);

        assert_eq!(
            root_from_task, root_from_base,
            "State root mismatch: task={root_from_task:?}, base={root_from_base:?}"
        );
    }
}
