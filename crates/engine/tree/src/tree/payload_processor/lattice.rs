//! Streaming lattice state root task.

use crate::tree::StateProviderBuilder;
use alloy_primitives::{keccak256, map::B256Map, B256};
use crossbeam_channel::Receiver as CrossbeamReceiver;
use reth_primitives_traits::{Account, NodePrimitives};
use reth_provider::{
    BlockReader, StateProviderBox, StateProviderFactory, StateReader, StateRootProvider,
};
use reth_trie::lattice::{
    LatticeAccumulatorUpdates, LatticeHashState, LatticeStateRoot, LatticeStorageRoot,
};
use reth_trie_parallel::{
    root::ParallelStateRootError,
    state_root_task::{LatticeRootComputeOutcome, LatticeRootMessage},
};
use revm_state::{Account as RevmAccount, AccountInfo, EvmState};
use std::sync::Arc;

/// Runs the streaming lattice root task until all state updates have been received.
pub(crate) fn run_lattice_root_task<N, P>(
    provider_builder: StateProviderBuilder<N, P>,
    updates: CrossbeamReceiver<LatticeRootMessage>,
) -> Result<LatticeRootComputeOutcome, ParallelStateRootError>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone,
{
    let provider = provider_builder.build()?;
    let mut task = LatticeRootTask::new(provider)?;

    loop {
        match updates.recv() {
            Ok(LatticeRootMessage::StateUpdate(update)) => task.apply_state_update(update)?,
            Ok(LatticeRootMessage::FinishedStateUpdates) => return task.finish(),
            Err(_) => {
                return Err(ParallelStateRootError::Other(
                    "lattice root update stream closed".to_string(),
                ))
            }
        }
    }
}

struct LatticeRootTask {
    provider: StateProviderBox,
    state_root: LatticeStateRoot,
    storage_roots: B256Map<LatticeStorageRoot>,
    storage_updates: B256Map<Option<LatticeHashState>>,
}

impl LatticeRootTask {
    fn new(provider: StateProviderBox) -> Result<Self, ParallelStateRootError> {
        let seed = provider.lattice_accumulator_seed()?;
        let state_root = LatticeStateRoot::from_state(&seed.state).map_err(lattice_error)?;
        let mut storage_roots = B256Map::default();

        for (hashed_address, storage_state) in &seed.storage {
            let storage_root = match storage_state {
                Some(state) => LatticeStorageRoot::from_state(state).map_err(lattice_error)?,
                None => LatticeStorageRoot::default(),
            };
            storage_roots.insert(*hashed_address, storage_root);
        }

        Ok(Self { provider, state_root, storage_roots, storage_updates: seed.storage })
    }

    fn apply_state_update(&mut self, update: EvmState) -> Result<(), ParallelStateRootError> {
        let mut accounts = update.into_iter().collect::<Vec<_>>();
        accounts.sort_unstable_by_key(|(address, _)| keccak256(address));

        for (address, account) in accounts {
            if !account.is_touched() {
                continue
            }

            let hashed_address = keccak256(address);
            let (old_storage_root, new_storage_root, storage_update) = {
                let storage_root = self.storage_root(hashed_address)?;
                let old_storage_root = storage_root.root();

                if account.is_selfdestructed() {
                    storage_root.reset();
                    for (slot, value) in &account.storage {
                        let present_value = value.present_value();
                        if !present_value.is_zero() {
                            storage_root
                                .add_slot(keccak256(slot.to_be_bytes::<32>()), present_value);
                        }
                    }
                } else {
                    for (slot, value) in account.changed_storage_slots() {
                        let original_value = value.original_value();
                        let present_value = value.present_value();
                        if original_value == present_value {
                            continue
                        }

                        let hashed_slot = keccak256(slot.to_be_bytes::<32>());
                        if !original_value.is_zero() {
                            storage_root.subtract_slot(hashed_slot, original_value);
                        }
                        if !present_value.is_zero() {
                            storage_root.add_slot(hashed_slot, present_value);
                        }
                    }
                }

                let new_storage_root = storage_root.root();
                let storage_update = (old_storage_root != new_storage_root ||
                    account.is_selfdestructed())
                .then(|| (!storage_root.is_zero()).then_some(storage_root.state()));

                (old_storage_root, new_storage_root, storage_update)
            };

            let old_account = old_account(&account);
            let new_account = new_account(&account);

            if old_account != new_account || old_storage_root != new_storage_root {
                if let Some(old_account) = old_account {
                    self.state_root.subtract_account(hashed_address, old_account, old_storage_root);
                }
                if let Some(new_account) = new_account {
                    self.state_root.add_account(hashed_address, new_account, new_storage_root);
                }
            }

            if let Some(storage_update) = storage_update {
                self.storage_updates.insert(hashed_address, storage_update);
            }
        }

        Ok(())
    }

    fn storage_root(
        &mut self,
        hashed_address: B256,
    ) -> Result<&mut LatticeStorageRoot, ParallelStateRootError> {
        if !self.storage_roots.contains_key(&hashed_address) {
            let storage_root = match self.provider.lattice_storage_accumulator(hashed_address)? {
                Some(state) => LatticeStorageRoot::from_state(&state).map_err(lattice_error)?,
                None => LatticeStorageRoot::default(),
            };
            self.storage_roots.insert(hashed_address, storage_root);
        }

        Ok(self.storage_roots.get_mut(&hashed_address).expect("storage root exists"))
    }

    fn finish(self) -> Result<LatticeRootComputeOutcome, ParallelStateRootError> {
        let state_root = self.state_root.root();
        let accumulator_updates =
            LatticeAccumulatorUpdates::new(self.state_root.state(), self.storage_updates);

        Ok(LatticeRootComputeOutcome {
            state_root,
            accumulator_updates: Arc::new(accumulator_updates),
        })
    }
}

fn old_account(account: &RevmAccount) -> Option<Account> {
    let original = account.original_info();
    existing_account(original)
        .filter(|_| !account.is_created() && !account.is_loaded_as_not_existing())
}

fn new_account(account: &RevmAccount) -> Option<Account> {
    if account.is_selfdestructed() {
        return None
    }

    existing_account(account.info.clone())
}

fn existing_account(info: AccountInfo) -> Option<Account> {
    (!info.is_empty()).then(|| info.into())
}

fn lattice_error(err: &'static str) -> ParallelStateRootError {
    ParallelStateRootError::Other(err.to_string())
}
