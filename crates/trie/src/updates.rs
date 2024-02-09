use derive_more::Deref;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{
    trie::{
        BranchNodeCompact, Nibbles, StorageTrieEntry, StoredBranchNode, StoredNibbles,
        StoredNibblesSubKey,
    },
    B256,
};
use std::collections::{hash_map::IntoIter, HashMap};

/// The key of a trie node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrieKey {
    /// A node in the account trie.
    AccountNode(StoredNibbles),
    /// A node in the storage trie.
    StorageNode(B256, StoredNibblesSubKey),
    /// Storage trie of an account.
    StorageTrie(B256),
}

/// The operation to perform on the trie.
#[derive(PartialEq, Eq, Debug, Clone)]
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
#[derive(Debug, Default, Clone, PartialEq, Eq, Deref)]
pub struct TrieUpdates {
    trie_operations: HashMap<TrieKey, TrieOp>,
}

impl<const N: usize> From<[(TrieKey, TrieOp); N]> for TrieUpdates {
    fn from(value: [(TrieKey, TrieOp); N]) -> Self {
        Self { trie_operations: HashMap::from(value) }
    }
}

impl IntoIterator for TrieUpdates {
    type Item = (TrieKey, TrieOp);
    type IntoIter = IntoIter<TrieKey, TrieOp>;

    fn into_iter(self) -> Self::IntoIter {
        self.trie_operations.into_iter()
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

    /// Extend the updates with trie updates.
    pub fn extend(&mut self, updates: impl Iterator<Item = (TrieKey, TrieOp)>) {
        self.trie_operations.extend(updates);
    }

    /// Extend the updates with account trie updates.
    pub fn extend_with_account_updates(&mut self, updates: HashMap<Nibbles, BranchNodeCompact>) {
        self.extend(
            updates.into_iter().map(|(nibbles, node)| {
                (TrieKey::AccountNode(nibbles.into()), TrieOp::Update(node))
            }),
        );
    }

    /// Extend the updates with storage trie updates.
    pub fn extend_with_storage_updates(
        &mut self,
        hashed_address: B256,
        updates: HashMap<Nibbles, BranchNodeCompact>,
    ) {
        self.extend(updates.into_iter().map(|(nibbles, node)| {
            (TrieKey::StorageNode(hashed_address, nibbles.into()), TrieOp::Update(node))
        }));
    }

    /// Extend the updates with deletes.
    pub fn extend_with_deletes(&mut self, keys: impl Iterator<Item = TrieKey>) {
        self.extend(keys.map(|key| (key, TrieOp::Delete)));
    }

    /// Flush updates all aggregated updates to the database.
    pub fn flush(self, tx: &(impl DbTx + DbTxMut)) -> Result<(), reth_db::DatabaseError> {
        if self.trie_operations.is_empty() {
            return Ok(())
        }

        let mut account_trie_cursor = tx.cursor_write::<tables::AccountsTrie>()?;
        let mut storage_trie_cursor = tx.cursor_dup_write::<tables::StoragesTrie>()?;

        let mut trie_operations = Vec::from_iter(self.trie_operations);
        trie_operations.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        for (key, operation) in trie_operations {
            match key {
                TrieKey::AccountNode(nibbles) => match operation {
                    TrieOp::Delete => {
                        if account_trie_cursor.seek_exact(nibbles)?.is_some() {
                            account_trie_cursor.delete_current()?;
                        }
                    }
                    TrieOp::Update(node) => {
                        if !nibbles.0.is_empty() {
                            account_trie_cursor.upsert(nibbles, StoredBranchNode(node))?;
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
                    if !nibbles.is_empty() {
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
