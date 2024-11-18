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
    /// Channels to retrieve proof calculation results from.
    pending_proofs: VecDeque<Receiver<Result<MultiProof, ParallelStateRootError>>>,
}

#[allow(dead_code)]
impl<Factory> StateRootTask<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + Send + Sync + 'static,
{
    /// Creates a new `StateRootTask`.
    pub(crate) fn new(config: StateRootConfig<Factory>, state_stream: StdReceiverStream) -> Self {
        Self { config, state_stream, state: Default::default(), pending_proofs: Default::default() }
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
        pending_proofs: &mut VecDeque<Receiver<Result<MultiProof, ParallelStateRootError>>>,
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
            let result = ParallelProof::new(view, input).multiproof(targets);
            let _ = tx.send(result);
        });

        pending_proofs.push_back(rx);

        state.extend(hashed_state_update);
    }

    fn run(mut self) -> StateRootResult {
        let mut task_state = StateRootTaskState::Idle(MultiProof::default(), B256::default());
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
                        &mut self.state,
                        &mut self.pending_proofs,
                    );
                    continue;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // no new state updates available, continue with other operations
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    // state stream closed, check if we can finish
                    if self.pending_proofs.is_empty() {
                        if let StateRootTaskState::Idle(_multiproof, state_root) = &task_state {
                            return Ok((*state_root, trie_updates));
                        }
                    }
                }
            }

            // check pending proofs
            while let Some(proof_rx) = self.pending_proofs.front() {
                match proof_rx.try_recv() {
                    Ok(result) => {
                        let multiproof = result?;
                        task_state.add_proofs(multiproof);
                        self.pending_proofs.pop_front();
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
                                continue;
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
                    debug!(target: "engine::root", accounts_len = self.state.accounts.len(), "Spawning state root calculation from proofs task");
                    let view = self.config.consistent_view.clone();
                    let input_nodes_sorted = self.config.input.nodes.clone().into_sorted();
                    let input_state_sorted = self.config.input.state.clone().into_sorted();
                    let multiproof = std::mem::take(multiproof);
                    let state = self.state.clone();
                    let (tx, rx) = mpsc::sync_channel(1);

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
                    continue;
                }
            }
        }
    }
}

fn calculate_state_root_from_proofs<Factory>(
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
            Ok(target_nodes(key.clone(), value, None, proof)?)
        })
        .collect::<Vec<ProviderResult<BTreeMap<_, _>>>>()
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<BTreeMap<_, _>>();

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

#[cfg(test)]
mod tests {
    use super::*;
    use reth_provider::{
        providers::ConsistentDbView,
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        ProviderFactory,
    };
    use reth_trie::TrieInput;
    use revm_primitives::{
        Account, AccountInfo, AccountStatus, Address, EvmState, EvmStorage, EvmStorageSlot,
        HashMap, B256, U256,
    };
    use std::sync::Arc;

    fn create_mock_config() -> StateRootConfig<ProviderFactory<MockNodeTypesWithDB>> {
        let factory = create_test_provider_factory();
        let view = ConsistentDbView::new(factory, None);
        let input = Arc::new(TrieInput::default());
        StateRootConfig { consistent_view: view, input }
    }

    fn create_mock_state() -> revm_primitives::EvmState {
        let mut state_changes: EvmState = HashMap::default();
        let storage = EvmStorage::from_iter([(U256::from(1), EvmStorageSlot::new(U256::from(2)))]);
        let account = Account {
            info: AccountInfo {
                balance: U256::from(100),
                nonce: 10,
                code_hash: B256::random(),
                code: Default::default(),
            },
            storage,
            status: AccountStatus::Loaded,
        };

        let address = Address::random();
        state_changes.insert(address, account);

        state_changes
    }

    #[test]
    fn test_state_root_task() {
        let config = create_mock_config();
        let (tx, rx) = std::sync::mpsc::channel();
        let stream = StdReceiverStream::new(rx);

        let task = StateRootTask::new(config, stream);
        let handle = task.spawn();

        for _ in 0..10 {
            tx.send(create_mock_state()).expect("failed to send state");
        }
        drop(tx);

        let result = handle.wait_for_result();
        assert!(result.is_ok(), "sync block execution failed");
    }
}
