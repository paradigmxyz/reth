//! State conversion at the evm2 execution boundary.

use alloc::vec::Vec;
use alloy_primitives::B256;
use reth_primitives_traits::Account;
use reth_trie_common::{keccak256_batch, HashedPostState, HashedStorage};

pub use evm2::block::{AccountUpdate, BlockState, BundleSource, StateUpdate, TransactionChanges};

/// Hashes a transaction's state update into the trie update consumed by state root tasks.
pub fn state_update_to_hashed_post_state(update: &StateUpdate) -> HashedPostState {
    let mut hashed_state = HashedPostState::with_capacity(update.accounts.len());

    // Hash every address and every storage slot of the update in two batches rather than one by
    // one. Storage of destroyed accounts is skipped below, so it is not hashed.
    let addresses: Vec<_> = update.accounts.iter().map(|account| account.address).collect();
    let hashed_addresses = keccak256_batch(&addresses);
    let slots: Vec<B256> = update
        .accounts
        .iter()
        .filter(|account| account.current.is_some())
        .flat_map(|account| account.storage.iter().map(|(key, _)| B256::from(*key)))
        .collect();
    let mut hashed_slots = keccak256_batch(&slots).into_iter();

    for (account, hashed_address) in update.accounts.iter().zip(hashed_addresses) {
        let destroyed = account.current.is_none();
        let info = account.current.clone().unwrap_or_default();
        let unchanged = account.original.as_ref() == Some(&info);
        // A deletion is explicit: an account that ends up empty is still present unless the
        // producer removed it, as EIP-161 does from Spurious Dragon on.
        if destroyed {
            hashed_state.accounts.insert(hashed_address, None);
        } else if !unchanged {
            hashed_state.accounts.insert(hashed_address, Some(Account::from(&info)));
        }
        if !destroyed && !account.storage.is_empty() {
            hashed_state.storages.insert(
                hashed_address,
                HashedStorage::from_iter(
                    account
                        .storage
                        .iter()
                        .zip(hashed_slots.by_ref())
                        .map(|((_, slot), hashed_slot)| (hashed_slot, slot.present_value)),
                ),
            );
        }
    }
    hashed_state
}
