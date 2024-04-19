use derive_more::Deref;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{
    trie::{
        BranchNodeCompact, HashBuilder, Nibbles, StorageTrieEntry, StoredBranchNode, StoredNibbles,
        StoredNibblesSubKey,
    },
    B256,
};
use std::collections::{hash_map::IntoIter, HashMap, HashSet};

use crate::walker::TrieWalker;

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
    pub fn extend(&mut self, updates: impl IntoIterator<Item = (TrieKey, TrieOp)>) {
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

    /// Finalize state trie updates.
    pub fn finalize_state_updates<C>(
        &mut self,
        walker: TrieWalker<C>,
        hash_builder: HashBuilder,
        destroyed_accounts: HashSet<B256>,
    ) {
        // Add updates from trie walker.
        let (_, walker_updates) = walker.split();
        self.extend(walker_updates);

        // Add account node updates from hash builder.
        let (_, hash_builder_updates) = hash_builder.split();
        self.extend_with_account_updates(hash_builder_updates);

        // Add deleted storage tries for destroyed accounts.
        self.extend(
            destroyed_accounts.into_iter().map(|key| (TrieKey::StorageTrie(key), TrieOp::Delete)),
        );
    }

    /// Finalize storage trie updates for a given address.
    pub fn finalize_storage_updates<C>(
        &mut self,
        hashed_address: B256,
        walker: TrieWalker<C>,
        hash_builder: HashBuilder,
    ) {
        // Add updates from trie walker.
        let (_, walker_updates) = walker.split();
        self.extend(walker_updates);

        // Add storage node updates from hash builder.
        let (_, hash_builder_updates) = hash_builder.split();
        self.extend(hash_builder_updates.into_iter().map(|(nibbles, node)| {
            (TrieKey::StorageNode(hashed_address, nibbles.into()), TrieOp::Update(node))
        }));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashed_cursor::HashedPostStateCursorFactory, test_utils::storage_root_prehashed,
        HashedPostState, HashedStorage, StorageRoot,
    };
    use itertools::Itertools;
    use reth_primitives::{keccak256, Address, StorageEntry, U256};
    use reth_provider::test_utils::create_test_provider_factory;
    use std::collections::BTreeMap;

    fn compute_storage_root<TX: DbTx>(
        address: Address,
        tx: &TX,
        post_state: &HashedPostState,
    ) -> (B256, TrieUpdates) {
        let (root, _, updates) = StorageRoot::from_tx(tx, address)
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                tx,
                &post_state.clone().into_sorted(),
            ))
            .with_prefix_set(
                post_state
                    .construct_prefix_sets()
                    .storage_prefix_sets
                    .remove(&keccak256(address))
                    .unwrap(),
            )
            .root_with_updates()
            .unwrap();
        (root, updates)
    }

    fn print_trie_updates(updates: &TrieUpdates) {
        for (key, op) in updates.iter().sorted_unstable_by_key(|(key, _)| (*key).clone()) {
            match key {
                TrieKey::StorageNode(_, StoredNibblesSubKey(nibbles)) => match op {
                    TrieOp::Update(node) => {
                        println!(
                            "Updated node at key {nibbles:?}: state {:?} tree {:?} hash {:?} hashes len {}",
                            node.state_mask, node.tree_mask, node.hash_mask, node.hashes.len()
                        );
                    }
                    TrieOp::Delete => {
                        println!("Deleting node at key {nibbles:?}");
                    }
                },
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn trie_updates_across_multiple_iterations() {
        let address = Address::random();
        let hashed_address = keccak256(address);

        // Generate initial storage with prefixes from 0xa to 0xf down to specified depth
        let mut hashed_storage = BTreeMap::default();
        // let max_depth = 4;
        // for depth in 1..=max_depth {
        //     for mut prefix in CHILD_INDEX_RANGE.permutations(depth) {
        //         prefix.resize(64, 0);
        //         let hashed_slot = B256::from_slice(&Nibbles::from_nibbles(prefix).pack());
        //         hashed_storage.insert(hashed_slot, U256::from(1));
        //     }
        // }

        let factory = create_test_provider_factory();

        let mut post_state = HashedPostState::default().with_storages([(
            hashed_address,
            HashedStorage::from_iter(false, hashed_storage.clone()),
        )]);

        println!("Computing initial storage root");
        let (storage_root, initial_updates) =
            compute_storage_root(address, factory.provider().unwrap().tx_ref(), &post_state);
        assert_eq!(storage_root, storage_root_prehashed(hashed_storage.clone()));

        println!("Initial updates");
        print_trie_updates(&initial_updates);

        let mut modified_storage = BTreeMap::default();

        // 0x0fff...
        let mut modified_key_prefix = vec![0x0, 0xf, 0xf, 0xf];
        modified_key_prefix.resize(64, 0);

        // 0x0fff...0aa000000000000
        let mut modified_entry1 = modified_key_prefix.clone();
        modified_entry1[50] = 0xa;
        modified_entry1[51] = 0xa;
        modified_storage.insert(
            B256::from_slice(&Nibbles::from_nibbles(&modified_entry1).pack()),
            U256::from(1),
        );

        // 0x0fff...0aaa00000000000
        let mut modified_entry2 = modified_key_prefix.clone();
        modified_entry2[50] = 0xa;
        modified_entry2[51] = 0xa;
        modified_entry2[52] = 0xa;
        modified_storage.insert(
            B256::from_slice(&Nibbles::from_nibbles(&modified_entry2).pack()),
            U256::from(1),
        );

        // 0x0fff...0ab000000000000
        let mut modified_entry3 = modified_key_prefix.clone();
        modified_entry3[50] = 0xa;
        modified_entry3[51] = 0xb;
        let modified_entry3_key = B256::from_slice(&Nibbles::from_nibbles(&modified_entry3).pack());
        modified_storage.insert(modified_entry3_key, U256::from(1));

        // 0x0fff...0ba000000000000
        let mut modified_entry4 = modified_key_prefix.clone();
        modified_entry4[50] = 0xb;
        modified_entry4[51] = 0xa;
        modified_storage.insert(
            B256::from_slice(&Nibbles::from_nibbles(&modified_entry4).pack()),
            U256::from(1),
        );

        // Update main hashed storage.
        hashed_storage.extend(modified_storage.clone());

        // let post_state = HashedPostState::default().with_storages([(
        //     hashed_address,
        //     HashedStorage::from_iter(false, modified_storage.clone()),
        // )]);
        post_state
            .storages
            .get_mut(&hashed_address)
            .unwrap()
            .storage
            .extend(modified_storage.clone());

        println!("Computing storage root");
        let (storage_root, modified_updates) =
            compute_storage_root(address, factory.provider().unwrap().tx_ref(), &post_state);
        assert_eq!(storage_root, storage_root_prehashed(hashed_storage.clone()));

        println!("New updates");
        print_trie_updates(&modified_updates);

        modified_storage.insert(modified_entry3_key, U256::ZERO);

        // Update main hashed storage.
        hashed_storage.remove(&modified_entry3_key);

        post_state
            .storages
            .get_mut(&hashed_address)
            .unwrap()
            .storage
            .extend(modified_storage.clone());

        println!("Computing storage root 2");
        let (storage_root, modified_updates2) =
            compute_storage_root(address, factory.provider().unwrap().tx_ref(), &post_state);
        assert_eq!(storage_root, storage_root_prehashed(hashed_storage.clone()));

        println!("New updates 2");
        print_trie_updates(&modified_updates2);

        {
            let mut updates = initial_updates.clone();
            updates.extend(modified_updates);
            updates.extend(modified_updates2);

            let provider_rw = factory.provider_rw().unwrap();
            let mut hashed_storage_cursor =
                provider_rw.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
            for (hashed_slot, value) in &hashed_storage {
                hashed_storage_cursor
                    .upsert(hashed_address, StorageEntry { key: *hashed_slot, value: *value })
                    .unwrap();
            }
            updates.flush(provider_rw.tx_ref()).unwrap();
            provider_rw.commit().unwrap();
        }

        let storage_root =
            StorageRoot::from_tx(factory.provider().unwrap().tx_ref(), address).root().unwrap();
        assert_eq!(storage_root, storage_root_prehashed(hashed_storage.clone()));
    }
}
