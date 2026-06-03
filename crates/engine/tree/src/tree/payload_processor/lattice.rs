//! Streaming lattice state root task.

use crate::tree::StateProviderBuilder;
use alloy_primitives::{keccak256, map::B256Map, B256};
use crossbeam_channel::Receiver as CrossbeamReceiver;
use rayon::prelude::*;
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
    pending_accounts: B256Map<PendingAccountUpdate>,
    storage_updates: B256Map<Option<LatticeHashState>>,
}

impl LatticeRootTask {
    fn new(provider: StateProviderBox) -> Result<Self, ParallelStateRootError> {
        let seed = provider.lattice_accumulator_seed()?;
        let state_root = LatticeStateRoot::from_state(&seed.state).map_err(lattice_error)?;

        Ok(Self {
            provider,
            state_root,
            pending_accounts: B256Map::default(),
            storage_updates: seed.storage,
        })
    }

    fn apply_state_update(&mut self, update: EvmState) -> Result<(), ParallelStateRootError> {
        let mut accounts = update.into_iter().collect::<Vec<_>>();
        accounts.sort_unstable_by_key(|(address, _)| keccak256(address));

        for (address, account) in accounts {
            if !account.is_touched() {
                continue
            }

            let hashed_address = keccak256(address);
            let pending = self.pending_account(hashed_address, &account)?;
            pending.new_account = new_account(&account);

            if account.is_selfdestructed() {
                pending.storage_root.reset();
                pending.storage_reset = true;
                for (slot, value) in &account.storage {
                    let present_value = value.present_value();
                    if !present_value.is_zero() {
                        pending
                            .storage_root
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
                        pending.storage_root.subtract_slot(hashed_slot, original_value);
                    }
                    if !present_value.is_zero() {
                        pending.storage_root.add_slot(hashed_slot, present_value);
                    }
                }
            }
        }

        Ok(())
    }

    fn pending_account(
        &mut self,
        hashed_address: B256,
        account: &RevmAccount,
    ) -> Result<&mut PendingAccountUpdate, ParallelStateRootError> {
        if !self.pending_accounts.contains_key(&hashed_address) {
            let seeded_storage_state = self.storage_updates.remove(&hashed_address);
            let persist_storage_update = seeded_storage_state.is_some();
            let old_storage_state = match seeded_storage_state {
                Some(storage_state) => storage_state,
                None => self.provider.lattice_storage_accumulator(hashed_address)?,
            };
            let storage_root = match &old_storage_state {
                Some(state) => LatticeStorageRoot::from_state(state).map_err(lattice_error)?,
                None => LatticeStorageRoot::default(),
            };

            self.pending_accounts.insert(
                hashed_address,
                PendingAccountUpdate {
                    old_account: old_account(account),
                    new_account: new_account(account),
                    old_storage_state,
                    storage_root,
                    storage_reset: false,
                    persist_storage_update,
                },
            );
        }

        Ok(self.pending_accounts.get_mut(&hashed_address).expect("pending account exists"))
    }

    fn finish(self) -> Result<LatticeRootComputeOutcome, ParallelStateRootError> {
        let Self { provider: _, mut state_root, pending_accounts, mut storage_updates } = self;
        let mut account_updates = pending_accounts
            .into_iter()
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|(hashed_address, pending)| pending.finish(hashed_address))
            .collect::<Result<Vec<_>, _>>()?;
        account_updates.sort_unstable_by_key(|update| update.hashed_address);

        for update in account_updates {
            if update.old_account != update.new_account ||
                update.old_storage_root != update.new_storage_root
            {
                if let Some(old_account) = update.old_account {
                    state_root.subtract_account(
                        update.hashed_address,
                        old_account,
                        update.old_storage_root,
                    );
                }
                if let Some(new_account) = update.new_account {
                    state_root.add_account(
                        update.hashed_address,
                        new_account,
                        update.new_storage_root,
                    );
                }
            }

            if let Some(storage_update) = update.storage_update {
                storage_updates.insert(update.hashed_address, storage_update);
            }
        }

        let state_root_hash = state_root.root();
        let accumulator_updates =
            LatticeAccumulatorUpdates::new(state_root.state(), storage_updates);

        Ok(LatticeRootComputeOutcome {
            state_root: state_root_hash,
            accumulator_updates: Arc::new(accumulator_updates),
        })
    }
}

struct PendingAccountUpdate {
    old_account: Option<Account>,
    new_account: Option<Account>,
    old_storage_state: Option<LatticeHashState>,
    storage_root: LatticeStorageRoot,
    storage_reset: bool,
    persist_storage_update: bool,
}

struct FinishedAccountUpdate {
    hashed_address: B256,
    old_account: Option<Account>,
    new_account: Option<Account>,
    old_storage_root: B256,
    new_storage_root: B256,
    storage_update: Option<Option<LatticeHashState>>,
}

impl PendingAccountUpdate {
    fn finish(self, hashed_address: B256) -> Result<FinishedAccountUpdate, ParallelStateRootError> {
        let old_storage_root = storage_root_from_state(self.old_storage_state.as_ref())?;
        let new_storage_root = self.storage_root.root();
        let storage_update = (self.persist_storage_update ||
            self.storage_reset ||
            old_storage_root != new_storage_root)
            .then(|| (!self.storage_root.is_zero()).then_some(self.storage_root.state()));

        Ok(FinishedAccountUpdate {
            hashed_address,
            old_account: self.old_account,
            new_account: self.new_account,
            old_storage_root,
            new_storage_root,
            storage_update,
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

fn storage_root_from_state(
    state: Option<&LatticeHashState>,
) -> Result<B256, ParallelStateRootError> {
    match state {
        Some(state) => {
            let storage_root = LatticeStorageRoot::from_state(state).map_err(lattice_error)?;
            Ok(storage_root.root())
        }
        None => Ok(LatticeStorageRoot::default().root()),
    }
}

fn lattice_error(err: &'static str) -> ParallelStateRootError {
    ParallelStateRootError::Other(err.to_string())
}
