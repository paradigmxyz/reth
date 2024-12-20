//! Common root computation functions.

use crate::TrieAccount;
use alloc::vec::Vec;
use alloy_primitives::{keccak256, Address, B256, U256};
use alloy_rlp::Encodable;
use alloy_trie::HashBuilder;
use itertools::Itertools;
use nybbles::Nibbles;

/// Hashes and sorts account keys, then proceeds to calculating the root hash of the state
/// represented as MPT.
/// See [`state_root_unsorted`] for more info.
pub fn state_root_ref_unhashed<'a, A: Into<TrieAccount> + Clone + 'a>(
    state: impl IntoIterator<Item = (&'a Address, &'a A)>,
) -> B256 {
    state_root_unsorted(
        state.into_iter().map(|(address, account)| (keccak256(address), account.clone())),
    )
}

/// Hashes and sorts account keys, then proceeds to calculating the root hash of the state
/// represented as MPT.
/// See [`state_root_unsorted`] for more info.
pub fn state_root_unhashed<A: Into<TrieAccount>>(
    state: impl IntoIterator<Item = (Address, A)>,
) -> B256 {
    state_root_unsorted(state.into_iter().map(|(address, account)| (keccak256(address), account)))
}

/// Sorts the hashed account keys and calculates the root hash of the state represented as MPT.
/// See [`state_root`] for more info.
pub fn state_root_unsorted<A: Into<TrieAccount>>(
    state: impl IntoIterator<Item = (B256, A)>,
) -> B256 {
    state_root(state.into_iter().sorted_unstable_by_key(|(key, _)| *key))
}

/// Calculates the root hash of the state represented as MPT.
///
/// Corresponds to [geth's `deriveHash`](https://github.com/ethereum/go-ethereum/blob/6c149fd4ad063f7c24d726a73bc0546badd1bc73/core/genesis.go#L119).
///
/// # Panics
///
/// If the items are not in sorted order.
pub fn state_root<A: Into<TrieAccount>>(state: impl IntoIterator<Item = (B256, A)>) -> B256 {
    let mut hb = HashBuilder::default();
    let mut account_rlp_buf = Vec::new();
    for (hashed_key, account) in state {
        account_rlp_buf.clear();
        account.into().encode(&mut account_rlp_buf);
        hb.add_leaf(Nibbles::unpack(hashed_key), &account_rlp_buf);
    }
    hb.root()
}

/// Hashes storage keys, sorts them and them calculates the root hash of the storage trie.
/// See [`storage_root_unsorted`] for more info.
pub fn storage_root_unhashed(storage: impl IntoIterator<Item = (B256, U256)>) -> B256 {
    storage_root_unsorted(storage.into_iter().map(|(slot, value)| (keccak256(slot), value)))
}

/// Sorts and calculates the root hash of account storage trie.
/// See [`storage_root`] for more info.
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
        hb.add_leaf(Nibbles::unpack(hashed_slot), alloy_rlp::encode_fixed_size(&value).as_ref());
    }
    hb.root()
}
