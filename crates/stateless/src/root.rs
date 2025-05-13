// Copied and modified from ress: https://github.com/paradigmxyz/ress/blob/06bf2c4788e45b8fcbd640e38b6243e6f87c4d0e/crates/engine/src/tree/root.rs

use alloc::vec::Vec;
use alloy_primitives::B256;
use alloy_rlp::{Decodable, Encodable};
use itertools::Itertools;
use reth_trie_common::{
    HashedPostState, Nibbles, TrieAccount, EMPTY_ROOT_HASH, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_sparse::{errors::SparseStateTrieResult, SparseStateTrie, SparseTrie};

/// Calculates the post-execution state root by applying state changes to a sparse trie.
///
/// This function takes a [`SparseStateTrie`] with the pre-state and a [`HashedPostState`]
/// containing account and storage changes resulting from block execution (state diff).
///
/// It modifies the input `trie` in place to reflect these changes and then calculates the
/// final post-execution state root.
pub(crate) fn calculate_state_root(
    trie: &mut SparseStateTrie,
    state: HashedPostState,
) -> SparseStateTrieResult<B256> {
    // 1. Apply storage‑slot updates and compute each contract’s storage root
    //
    //
    // We walk over every (address, storage) pair in deterministic order
    // and update the corresponding per‑account storage trie in‑place.
    // When we’re done we collect (address, updated_storage_trie) in a `Vec`
    // so that we can insert them back into the outer state trie afterwards ― this avoids
    // borrowing issues.
    let mut storage_results = Vec::with_capacity(state.storages.len());

    for (address, storage) in state.storages.into_iter().sorted_unstable_by_key(|(addr, _)| *addr) {
        // Take the existing storage trie (or create an empty, “revealed” one)
        let mut storage_trie =
            trie.take_storage_trie(&address).unwrap_or_else(SparseTrie::revealed_empty);

        if storage.wiped {
            storage_trie.wipe()?;
        }

        // Apply slot‑level changes
        for (hashed_slot, value) in
            storage.storage.into_iter().sorted_unstable_by_key(|(slot, _)| *slot)
        {
            let nibbles = Nibbles::unpack(hashed_slot);
            if value.is_zero() {
                storage_trie.remove_leaf(&nibbles)?;
            } else {
                storage_trie.update_leaf(nibbles, alloy_rlp::encode_fixed_size(&value).to_vec())?;
            }
        }

        // Finalise the storage‑trie root before pushing the result
        storage_trie.root();
        storage_results.push((address, storage_trie));
    }

    // Insert every updated storage trie back into the outer state trie
    for (address, storage_trie) in storage_results {
        trie.insert_storage_trie(address, storage_trie);
    }

    // 2. Apply account‑level updates and (re)encode the account nodes
    // Update accounts with new values
    // TODO: upstream changes into reth so that `SparseStateTrie::update_account` handles this
    let mut account_rlp_buf = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);

    for (hashed_address, account) in
        state.accounts.into_iter().sorted_unstable_by_key(|(addr, _)| *addr)
    {
        let nibbles = Nibbles::unpack(hashed_address);
        let account = account.unwrap_or_default();

        // Determine which storage root should be used for this account
        let storage_root = if let Some(storage_trie) = trie.storage_trie_mut(&hashed_address) {
            storage_trie.root()
        } else if let Some(value) = trie.get_account_value(&hashed_address) {
            TrieAccount::decode(&mut &value[..])?.storage_root
        } else {
            EMPTY_ROOT_HASH
        };

        // Decide whether to remove or update the account leaf
        if account.is_empty() && storage_root == EMPTY_ROOT_HASH {
            trie.remove_account_leaf(&nibbles)?;
        } else {
            account_rlp_buf.clear();
            account.into_trie_account(storage_root).encode(&mut account_rlp_buf);
            trie.update_account_leaf(nibbles, account_rlp_buf.clone())?;
        }
    }

    // Return new state root
    trie.root()
}
