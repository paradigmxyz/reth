use crate::{
    trie_cursor::{TrieCursor, TrieCursorMut, TrieCursorRwFactory, TrieDupCursorRw},
    walker::TrieWalker,
    BranchNodeCompact, HashBuilder, Nibbles,
};
use reth_primitives::B256;
use std::collections::{HashMap, HashSet};

/// The aggregation of trie updates.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrieUpdates {
    pub(crate) account_nodes: HashMap<Nibbles, BranchNodeCompact>,
    pub(crate) removed_nodes: HashSet<Nibbles>,
    pub(crate) storage_tries: HashMap<B256, StorageTrieUpdates>,
}

#[cfg(feature = "test-utils")]
impl TrieUpdates {
    /// Changed storage tries.
    pub const fn storage_tries(&self) -> &HashMap<B256, StorageTrieUpdates> {
        &self.storage_tries
    }

    /// Nodes removed from the trie.
    pub const fn removed_nodes(&self) -> &HashSet<Nibbles> {
        &self.removed_nodes
    }
}

impl TrieUpdates {
    /// Returns `true` if the updates are empty.
    pub fn is_empty(&self) -> bool {
        self.account_nodes.is_empty() &&
            self.removed_nodes.is_empty() &&
            self.storage_tries.is_empty()
    }

    /// Returns reference to updated account nodes.
    pub const fn account_nodes_ref(&self) -> &HashMap<Nibbles, BranchNodeCompact> {
        &self.account_nodes
    }

    /// Returns a reference to removed account nodes.
    pub const fn removed_nodes_ref(&self) -> &HashSet<Nibbles> {
        &self.removed_nodes
    }

    /// Returns a reference to updated storage tries.
    pub const fn storage_tries_ref(&self) -> &HashMap<B256, StorageTrieUpdates> {
        &self.storage_tries
    }

    /// Insert storage updates for a given hashed address.
    pub fn insert_storage_updates(
        &mut self,
        hashed_address: B256,
        storage_updates: StorageTrieUpdates,
    ) {
        let existing = self.storage_tries.insert(hashed_address, storage_updates);
        debug_assert!(existing.is_none());
    }

    /// Finalize state trie updates.
    pub fn finalize<C>(
        &mut self,
        walker: TrieWalker<C>,
        hash_builder: HashBuilder,
        destroyed_accounts: HashSet<B256>,
    ) {
        // Retrieve deleted keys from trie walker.
        let (_, removed_node_keys) = walker.split();
        self.removed_nodes.extend(removed_node_keys);

        // Retrieve updated nodes from hash builder.
        let (_, updated_nodes) = hash_builder.split();
        self.account_nodes.extend(updated_nodes);

        // Add deleted storage tries for destroyed accounts.
        for destroyed in destroyed_accounts {
            self.storage_tries.entry(destroyed).or_default().set_deleted(true);
        }
    }

    /// Converts trie updates into [`TrieUpdatesSorted`].
    pub fn into_sorted(self) -> TrieUpdatesSorted {
        let mut account_nodes = Vec::from_iter(self.account_nodes);
        account_nodes.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let storage_tries = self
            .storage_tries
            .into_iter()
            .map(|(hashed_address, updates)| (hashed_address, updates.into_sorted()))
            .collect();
        TrieUpdatesSorted { removed_nodes: self.removed_nodes, account_nodes, storage_tries }
    }

    /// Flush updates all aggregated updates to the database.
    ///
    /// # Returns
    ///
    /// The number of storage trie entries updated in the database.
    pub fn write_to_database<F>(self, factory: &F) -> Result<usize, F::Err>
    where
        F: TrieCursorRwFactory,
    {
        if self.is_empty() {
            return Ok(0)
        }

        // Track the number of inserted entries.
        let mut num_entries = 0;

        // Merge updated and removed nodes. Updated nodes must take precedence.
        let mut account_updates = self
            .removed_nodes
            .into_iter()
            .filter_map(|n| (!self.account_nodes.contains_key(&n)).then_some((n, None)))
            .collect::<Vec<_>>();
        account_updates
            .extend(self.account_nodes.into_iter().map(|(nibbles, node)| (nibbles, Some(node))));
        // Sort trie node updates.
        account_updates.sort_unstable_by(|a, b| a.0.cmp(&b.0));

        let mut account_trie_cursor = factory.account_trie_cursor()?;
        for (nibbles, updated_node) in account_updates {
            match updated_node {
                Some(node) => {
                    if !nibbles.is_empty() {
                        num_entries += 1;
                        account_trie_cursor.upsert(nibbles, node)?;
                    }
                }
                None => {
                    num_entries += 1;
                    if account_trie_cursor.seek_exact(nibbles)?.is_some() {
                        account_trie_cursor.delete_current()?;
                    }
                }
            }
        }

        let mut storage_tries = Vec::from_iter(self.storage_tries);
        storage_tries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut storage_trie_cursor = factory.storage_trie_cursor()?;
        for (hashed_address, storage_trie_updates) in storage_tries {
            let updated_storage_entries =
                storage_trie_updates.write_with_cursor(&mut storage_trie_cursor, hashed_address)?;
            num_entries += updated_storage_entries;
        }

        Ok(num_entries)
    }
}

/// Trie updates for storage trie of a single account.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StorageTrieUpdates {
    /// Flag indicating whether the trie was deleted.
    pub(crate) is_deleted: bool,
    /// Collection of updated storage trie nodes.
    pub(crate) storage_nodes: HashMap<Nibbles, BranchNodeCompact>,
    /// Collection of removed storage trie nodes.
    pub(crate) removed_nodes: HashSet<Nibbles>,
}

#[cfg(feature = "test-utils")]
impl StorageTrieUpdates {
    /// Creates a new storage trie updates that are not marked as deleted.
    pub fn new(updates: HashMap<Nibbles, BranchNodeCompact>) -> Self {
        Self { storage_nodes: updates, ..Default::default() }
    }

    /// Changed storage nodes.
    pub const fn storage_nodes(&self) -> &HashMap<Nibbles, BranchNodeCompact> {
        &self.storage_nodes
    }

    /// Removed storage nodes.
    pub const fn removed_nodes(&self) -> &HashSet<Nibbles> {
        &self.removed_nodes
    }
}

impl StorageTrieUpdates {
    /// Returns empty storage trie updates with `deleted` set to `true`.
    pub fn deleted() -> Self {
        Self {
            is_deleted: true,
            storage_nodes: HashMap::default(),
            removed_nodes: HashSet::default(),
        }
    }

    /// Returns the length of updated nodes.
    pub fn len(&self) -> usize {
        (self.is_deleted as usize) + self.storage_nodes.len() + self.removed_nodes.len()
    }

    /// Returns `true` if the trie was deleted.
    pub const fn is_deleted(&self) -> bool {
        self.is_deleted
    }

    /// Returns reference to updated storage nodes.
    pub const fn storage_nodes_ref(&self) -> &HashMap<Nibbles, BranchNodeCompact> {
        &self.storage_nodes
    }

    /// Returns reference to removed storage nodes.
    pub const fn removed_nodes_ref(&self) -> &HashSet<Nibbles> {
        &self.removed_nodes
    }

    /// Returns `true` if storage updates are empty.
    pub fn is_empty(&self) -> bool {
        !self.is_deleted && self.storage_nodes.is_empty() && self.removed_nodes.is_empty()
    }

    /// Sets `deleted` flag on the storage trie.
    pub fn set_deleted(&mut self, deleted: bool) {
        self.is_deleted = deleted;
    }

    /// Finalize storage trie updates for by taking updates from walker and hash builder.
    pub fn finalize<C>(&mut self, walker: TrieWalker<C>, hash_builder: HashBuilder) {
        // Retrieve deleted keys from trie walker.
        let (_, removed_keys) = walker.split();
        self.removed_nodes.extend(removed_keys);

        // Retrieve updated nodes from hash builder.
        let (_, updated_nodes) = hash_builder.split();
        self.storage_nodes.extend(updated_nodes);
    }

    /// Convert storage trie updates into [`StorageTrieUpdatesSorted`].
    pub fn into_sorted(self) -> StorageTrieUpdatesSorted {
        let mut storage_nodes = Vec::from_iter(self.storage_nodes);
        storage_nodes.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        StorageTrieUpdatesSorted {
            is_deleted: self.is_deleted,
            removed_nodes: self.removed_nodes,
            storage_nodes,
        }
    }

    /// Initializes a storage trie cursor and writes updates to database.
    pub fn write_to_database<F>(self, factory: &F, hashed_address: B256) -> Result<usize, F::Err>
    where
        F: TrieCursorRwFactory,
    {
        if self.is_empty() {
            return Ok(0)
        }

        let mut cursor = factory.storage_trie_cursor()?;
        self.write_with_cursor(&mut cursor, hashed_address)
    }

    /// Writes updates to database.
    ///
    /// # Returns
    ///
    /// The number of storage trie entries updated in the database.
    fn write_with_cursor<C, Err>(self, cursor: &mut C, hashed_address: B256) -> Result<usize, Err>
    where
        C: TrieDupCursorRw<Err, Err>,
    {
        // The storage trie for this account has to be deleted.
        if self.is_deleted && cursor.seek_exact(hashed_address)?.is_some() {
            cursor.delete_current_duplicates()?;
        }

        // Merge updated and removed nodes. Updated nodes must take precedence.
        let mut storage_updates = self
            .removed_nodes
            .into_iter()
            .filter_map(|n| (!self.storage_nodes.contains_key(&n)).then_some((n, None)))
            .collect::<Vec<_>>();
        storage_updates
            .extend(self.storage_nodes.into_iter().map(|(nibbles, node)| (nibbles, Some(node))));
        // Sort trie node updates.
        storage_updates.sort_unstable_by(|a, b| a.0.cmp(&b.0));

        let mut num_entries = 0;
        for (nibbles, maybe_updated) in storage_updates.into_iter().filter(|(n, _)| !n.is_empty()) {
            num_entries += 1;
            // Delete the old entry if it exists.
            if cursor
                .seek_by_key_subkey(hashed_address, nibbles.clone())?
                .filter(|(key, _)| *key == nibbles)
                .is_some()
            {
                cursor.delete_current()?;
            }

            // There is an updated version of this node, insert new entry.
            if let Some(node) = maybe_updated {
                cursor.upsert(hashed_address, nibbles, node)?;
            }
        }

        Ok(num_entries)
    }
}

/// Sorted trie updates used for lookups and insertions.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
pub struct TrieUpdatesSorted {
    pub(crate) account_nodes: Vec<(Nibbles, BranchNodeCompact)>,
    pub(crate) removed_nodes: HashSet<Nibbles>,
    pub(crate) storage_tries: HashMap<B256, StorageTrieUpdatesSorted>,
}

impl TrieUpdatesSorted {
    /// Returns reference to updated account nodes.
    pub fn account_nodes_ref(&self) -> &[(Nibbles, BranchNodeCompact)] {
        &self.account_nodes
    }

    /// Returns reference to removed account nodes.
    pub const fn removed_nodes_ref(&self) -> &HashSet<Nibbles> {
        &self.removed_nodes
    }

    /// Returns reference to updated storage tries.
    pub const fn storage_tries_ref(&self) -> &HashMap<B256, StorageTrieUpdatesSorted> {
        &self.storage_tries
    }
}

/// Sorted trie updates used for lookups and insertions.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
pub struct StorageTrieUpdatesSorted {
    pub(crate) is_deleted: bool,
    pub(crate) storage_nodes: Vec<(Nibbles, BranchNodeCompact)>,
    pub(crate) removed_nodes: HashSet<Nibbles>,
}

impl StorageTrieUpdatesSorted {
    /// Returns `true` if the trie was deleted.
    pub const fn is_deleted(&self) -> bool {
        self.is_deleted
    }

    /// Returns reference to updated storage nodes.
    pub fn storage_nodes_ref(&self) -> &[(Nibbles, BranchNodeCompact)] {
        &self.storage_nodes
    }

    /// Returns reference to removed storage nodes.
    pub const fn removed_nodes_ref(&self) -> &HashSet<Nibbles> {
        &self.removed_nodes
    }
}
