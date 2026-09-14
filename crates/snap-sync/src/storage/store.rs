//! Checks storage dependencies against the roots required by account-range persistence.

use crate::SnapSyncError;
use alloy_primitives::B256;
use reth_trie_common::{root::storage_root, HashedPostStateSorted, TrieAccount, EMPTY_ROOT_HASH};

// Whether `account` has storage, which must be supplied and hash to its root.
pub(crate) fn verify_storage(
    state: &HashedPostStateSorted,
    hash: B256,
    account: &TrieAccount,
) -> Result<bool, SnapSyncError> {
    if account.storage_root == EMPTY_ROOT_HASH {
        return Ok(false)
    }
    let storage = state
        .account_storages()
        .get(&hash)
        .ok_or(SnapSyncError::MissingStorage { account: hash })?;
    // Zero slots are deletions, which the trie does not hold.
    let got = storage_root(
        storage.storage_slots_ref().iter().filter(|(_, value)| !value.is_zero()).copied(),
    );
    if got != account.storage_root {
        return Err(SnapSyncError::StorageRootMismatch {
            account: hash,
            expected: account.storage_root,
            got,
        })
    }
    Ok(true)
}
