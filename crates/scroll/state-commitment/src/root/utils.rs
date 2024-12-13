//! Utilities for computing state root and storage root from a plain provided state.

use crate::{PoseidonKeyHasher, PoseidonValueHasher, ScrollTrieAccount};
use alloy_primitives::{Address, B256, U256};
use itertools::Itertools;
use reth_scroll_trie::HashBuilder;
use reth_trie::{BitsCompatibility, KeyHasher, Nibbles};

/// Hashes and sorts account keys, then proceeds to calculating the root hash of the state
/// represented as MPT.
pub fn state_root_ref_unhashed<'a, A: Into<ScrollTrieAccount> + Clone + 'a>(
    state: impl IntoIterator<Item = (&'a Address, &'a A)>,
) -> B256 {
    state_root_unsorted(
        state
            .into_iter()
            .map(|(address, account)| (PoseidonKeyHasher::hash_key(address), account.clone())),
    )
}

/// Sorts the hashed account keys and calculates the root hash of the state represented as MPT.
pub fn state_root_unsorted<A: Into<ScrollTrieAccount>>(
    state: impl IntoIterator<Item = (B256, A)>,
) -> B256 {
    state_root(state.into_iter().sorted_unstable_by_key(|(key, _)| *key))
}

/// Calculates the root hash of the state represented as BMPT.
///
/// # Panics
///
/// If the items are not in sorted order.
pub fn state_root<A: Into<ScrollTrieAccount>>(state: impl IntoIterator<Item = (B256, A)>) -> B256 {
    let mut hb = HashBuilder::default();
    for (hashed_key, account) in state {
        let account_hash = PoseidonValueHasher::hash_account(account.into());
        hb.add_leaf(Nibbles::unpack_bits(hashed_key), account_hash.as_slice());
    }
    hb.root()
}

/// Hashes storage keys, sorts them and them calculates the root hash of the storage trie.
pub fn storage_root_unhashed(storage: impl IntoIterator<Item = (B256, U256)>) -> B256 {
    storage_root_unsorted(
        storage.into_iter().map(|(slot, value)| (PoseidonKeyHasher::hash_key(slot), value)),
    )
}

/// Sorts and calculates the root hash of account storage trie.
pub fn storage_root_unsorted(storage: impl IntoIterator<Item = (B256, U256)>) -> B256 {
    storage_root(storage.into_iter().sorted_unstable_by_key(|(key, _)| *key))
}

/// Calculates the root hash of account storage trie.
///
/// # Panics
///
/// If the items are not in sorted order.
pub fn storage_root(storage: impl IntoIterator<Item = (B256, U256)>) -> B256 {
    let mut hb = HashBuilder::default();
    for (hashed_slot, value) in storage {
        let hashed_value = PoseidonValueHasher::hash_storage(value);
        hb.add_leaf(Nibbles::unpack_bits(hashed_slot), hashed_value.as_ref());
    }
    hb.root()
}
