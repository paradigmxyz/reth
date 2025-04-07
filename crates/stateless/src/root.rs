// Copied from ress: https://github.com/paradigmxyz/ress/blob/06bf2c4788e45b8fcbd640e38b6243e6f87c4d0e/crates/engine/src/tree/root.rs

use alloy_primitives::B256;
use alloy_rlp::{Decodable, Encodable};
use itertools::Itertools;
use rayon::prelude::*;
use reth_trie::{
    HashedPostState, Nibbles, TrieAccount, EMPTY_ROOT_HASH, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_sparse::{errors::SparseStateTrieResult, SparseStateTrie, SparseTrie};
use std::sync::mpsc;

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
    // Update storage slots with new values and calculate storage roots.
    let (storage_tx, storage_rx) = mpsc::channel();

    state
        .storages
        .into_iter()
        .map(|(address, storage)| (address, storage, trie.take_storage_trie(&address)))
        .par_bridge()
        .map(|(address, storage, storage_trie)| {
            let mut storage_trie = storage_trie.unwrap_or_else(SparseTrie::revealed_empty);

            if storage.wiped {
                storage_trie.wipe()?;
            }
            for (hashed_slot, value) in
                storage.storage.into_iter().sorted_unstable_by_key(|(hashed_slot, _)| *hashed_slot)
            {
                let nibbles = Nibbles::unpack(hashed_slot);
                if value.is_zero() {
                    storage_trie.remove_leaf(&nibbles)?;
                } else {
                    storage_trie
                        .update_leaf(nibbles, alloy_rlp::encode_fixed_size(&value).to_vec())?;
                }
            }

            storage_trie.root();

            SparseStateTrieResult::Ok((address, storage_trie))
        })
        .for_each_init(
            || storage_tx.clone(),
            |storage_tx, result| storage_tx.send(result).unwrap(),
        );
    drop(storage_tx);
    for result in storage_rx {
        let (address, storage_trie) = result?;
        trie.insert_storage_trie(address, storage_trie);
    }

    // Update accounts with new values
    // TODO: upstream changes into reth so that `SparseStateTrie::update_account` handles this
    let mut account_rlp_buf = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
    for (hashed_address, account) in
        state.accounts.into_iter().sorted_unstable_by_key(|(hashed_address, _)| *hashed_address)
    {
        let nibbles = Nibbles::unpack(hashed_address);
        let account = account.unwrap_or_default();
        let storage_root = if let Some(storage_trie) = trie.storage_trie_mut(&hashed_address) {
            storage_trie.root()
        } else if let Some(value) = trie.get_account_value(&hashed_address) {
            TrieAccount::decode(&mut &value[..])?.storage_root
        } else {
            EMPTY_ROOT_HASH
        };

        if account.is_empty() && storage_root == EMPTY_ROOT_HASH {
            trie.remove_account_leaf(&nibbles)?;
        } else {
            account_rlp_buf.clear();
            account.into_trie_account(storage_root).encode(&mut account_rlp_buf);
            trie.update_account_leaf(nibbles, account_rlp_buf.clone())?;
        }
    }

    trie.root()
}
