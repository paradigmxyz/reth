use crate::Nibbles;
use derive_more::Deref;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{
    trie::{BranchNodeCompact, StorageTrieEntry, StoredNibbles, StoredNibblesSubKey},
    H256,
};
use std::collections::BTreeMap;

/// The key of a trie node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrieKey {
    /// A node in the account trie.
    AccountNode(StoredNibbles),
    /// A node in the storage trie.
    StorageNode(H256, StoredNibblesSubKey),
    /// Storage trie of an account.
    StorageTrie(H256),
}

/// The operation to perform on the trie.
#[derive(Debug, Clone)]
pub enum TrieOp {
    /// Delete the node entry.
    Delete,
    /// Update the node entry with the provided value.
    Update(BranchNodeCompact),
}

impl TrieOp {
    /// Returns `true` if the operation is an update.
    pub fn is_update(&self) -> bool {
        matches!(self, TrieOp::Update(..))
    }
}

/// The aggregation of trie updates.
#[derive(Debug, Default, Clone, Deref)]
pub struct TrieUpdates {
    trie_operations: BTreeMap<TrieKey, TrieOp>,
}

impl<const N: usize> From<[(TrieKey, TrieOp); N]> for TrieUpdates {
    fn from(value: [(TrieKey, TrieOp); N]) -> Self {
        Self { trie_operations: BTreeMap::from(value) }
    }
}

impl TrieUpdates {
    /// Schedule a delete operation on a trie key.
    ///
    /// # Panics
    ///
    /// If the key already exists and the operation is an update.
    pub fn schedule_delete(&mut self, key: TrieKey) {
        let existing = self.trie_operations.insert(key, TrieOp::Delete);
        if let Some(op) = existing {
            assert!(!op.is_update(), "Tried to delete a node that was already updated");
        }
    }

    /// Append the updates to the current updates.
    pub fn append(&mut self, other: &mut Self) {
        self.trie_operations.append(&mut other.trie_operations);
    }

    /// Extend the updates with trie updates.
    pub fn extend(&mut self, updates: impl Iterator<Item = (TrieKey, TrieOp)>) {
        self.trie_operations.extend(updates);
    }

    /// Extend the updates with account trie updates.
    pub fn extend_with_account_updates(&mut self, updates: BTreeMap<Nibbles, BranchNodeCompact>) {
        self.extend(updates.into_iter().map(|(nibbles, node)| {
            (TrieKey::AccountNode(nibbles.hex_data.into()), TrieOp::Update(node))
        }));
    }

    /// Extend the updates with storage trie updates.
    pub fn extend_with_storage_updates(
        &mut self,
        hashed_address: H256,
        updates: BTreeMap<Nibbles, BranchNodeCompact>,
    ) {
        self.extend(updates.into_iter().map(|(nibbles, node)| {
            (TrieKey::StorageNode(hashed_address, nibbles.hex_data.into()), TrieOp::Update(node))
        }));
    }

    /// Flush updates all aggregated updates to the database.
    pub fn flush<'a, 'tx, TX>(self, tx: &'a TX) -> Result<(), reth_db::Error>
    where
        TX: DbTx<'tx> + DbTxMut<'tx>,
    {
        if self.trie_operations.is_empty() {
            return Ok(())
        }

        let mut account_trie_cursor = tx.cursor_write::<tables::AccountsTrie>()?;
        let mut storage_trie_cursor = tx.cursor_dup_write::<tables::StoragesTrie>()?;

        for (key, operation) in self.trie_operations {
            match key {
                TrieKey::AccountNode(nibbles) => match operation {
                    TrieOp::Delete => {
                        if account_trie_cursor.seek_exact(nibbles)?.is_some() {
                            account_trie_cursor.delete_current()?;
                        }
                    }
                    TrieOp::Update(node) => {
                        if !nibbles.inner.is_empty() {
                            account_trie_cursor.upsert(nibbles, node)?;
                        }
                    }
                },
                TrieKey::StorageTrie(hashed_address) => match operation {
                    TrieOp::Delete => {
                        if storage_trie_cursor.seek_exact(hashed_address)?.is_some() {
                            storage_trie_cursor.delete_current_duplicates()?;
                        }
                    }
                    TrieOp::Update(..) => unreachable!("Cannot update full storage trie."),
                },
                TrieKey::StorageNode(hashed_address, nibbles) => {
                    if !nibbles.inner.is_empty() {
                        // Delete the old entry if it exists.
                        if storage_trie_cursor
                            .seek_by_key_subkey(hashed_address, nibbles.clone())?
                            .filter(|e| e.nibbles == nibbles)
                            .is_some()
                        {
                            storage_trie_cursor.delete_current()?;
                        }

                        // The operation is an update, insert new entry.
                        if let TrieOp::Update(node) = operation {
                            storage_trie_cursor
                                .upsert(hashed_address, StorageTrieEntry { nibbles, node })?;
                        }
                    }
                }
            };
        }

        Ok(())
    }
}
