//! State conversion at the evm2 execution boundary.

use alloy_primitives::{keccak256, B256};
use reth_primitives_traits::Account;
use reth_trie_common::{HashedPostState, HashedStorage};

pub use evm2::block::{AccountUpdate, BlockState, BundleSource, StateUpdate, TransactionChanges};

/// Hashes a transaction's state update into the trie update consumed by state root tasks.
pub fn state_update_to_hashed_post_state(update: &StateUpdate) -> HashedPostState {
    let mut hashed_state = HashedPostState::with_capacity(update.accounts.len());
    for account in &update.accounts {
        let hashed_address = keccak256(account.address);
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
                        .map(|(key, slot)| (keccak256(B256::from(*key)), slot.present_value)),
                ),
            );
        }
    }
    hashed_state
}
