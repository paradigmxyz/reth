//! Streaming lattice state root task.

use crate::tree::StateProviderBuilder;
use alloy_primitives::{
    keccak256,
    map::{B256Map, B256Set},
    B256, U256,
};
use crossbeam_channel::Receiver as CrossbeamReceiver;
use reth_primitives_traits::{Account, NodePrimitives};
use reth_provider::{
    BlockReader, StateProviderBox, StateProviderFactory, StateReader, StateRootProvider,
};
use reth_trie::lattice::{LatticeAccumulatorUpdates, LatticeRoot};
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
    lattice_root: LatticeRoot,
    storage_overrides: B256Map<B256Map<U256>>,
    wiped_storage_accounts: B256Set,
}

impl LatticeRootTask {
    fn new(provider: StateProviderBox) -> Result<Self, ParallelStateRootError> {
        let seed = provider.lattice_accumulator_seed()?;
        let lattice_root = LatticeRoot::from_state(&seed.state).map_err(lattice_error)?;

        Ok(Self {
            provider,
            lattice_root,
            storage_overrides: B256Map::default(),
            wiped_storage_accounts: B256Set::default(),
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
            let old_account = old_account(&account);
            let new_account = new_account(&account);

            if old_account != new_account {
                if let Some(old_account) = old_account {
                    self.lattice_root.subtract_account(hashed_address, old_account);
                }
                if let Some(new_account) = new_account {
                    self.lattice_root.add_account(hashed_address, new_account);
                }
            }

            if account.is_selfdestructed() {
                self.subtract_current_storage(hashed_address)?;
                for (slot, value) in &account.storage {
                    let present_value = value.present_value();
                    if !present_value.is_zero() {
                        let hashed_slot = keccak256(slot.to_be_bytes::<32>());
                        self.lattice_root.add_slot(hashed_address, hashed_slot, present_value);
                        self.storage_overrides
                            .entry(hashed_address)
                            .or_default()
                            .insert(hashed_slot, present_value);
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
                        self.lattice_root.subtract_slot(
                            hashed_address,
                            hashed_slot,
                            original_value,
                        );
                    }
                    if !present_value.is_zero() {
                        self.lattice_root.add_slot(hashed_address, hashed_slot, present_value);
                    }
                    self.storage_overrides
                        .entry(hashed_address)
                        .or_default()
                        .insert(hashed_slot, present_value);
                }
            }
        }

        Ok(())
    }

    fn subtract_current_storage(
        &mut self,
        hashed_address: B256,
    ) -> Result<(), ParallelStateRootError> {
        let base_storage = if self.wiped_storage_accounts.contains(&hashed_address) {
            B256Map::default()
        } else {
            self.provider
                .lattice_account_storage(hashed_address)?
                .into_iter()
                .collect::<B256Map<_>>()
        };
        let overrides = self.storage_overrides.remove(&hashed_address).unwrap_or_default();
        let mut current_storage = base_storage;

        for (hashed_slot, value) in overrides {
            if value.is_zero() {
                current_storage.remove(&hashed_slot);
            } else {
                current_storage.insert(hashed_slot, value);
            }
        }

        for (hashed_slot, value) in current_storage {
            self.lattice_root.subtract_slot(hashed_address, hashed_slot, value);
        }
        self.wiped_storage_accounts.insert(hashed_address);

        Ok(())
    }

    fn finish(self) -> Result<LatticeRootComputeOutcome, ParallelStateRootError> {
        let state_root = self.lattice_root.root();
        let accumulator_updates = LatticeAccumulatorUpdates::new(self.lattice_root.state());

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
