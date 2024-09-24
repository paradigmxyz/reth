use crate::{walker::TrieWalker, BranchNodeCompact, HashBuilder, Nibbles};
use alloy_primitives::B256;
#[cfg(feature = "serde")]
use serde::{ser::SerializeMap, Serialize, Serializer};
use std::collections::{HashMap, HashSet};

/// The aggregation of trie updates.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrieUpdates {
    #[cfg_attr(feature = "serde", serde(serialize_with = "serialize_nibbles_map"))]
    pub(crate) account_nodes: HashMap<Nibbles, BranchNodeCompact>,
    pub(crate) removed_nodes: HashSet<Nibbles>,
    pub(crate) storage_tries: HashMap<B256, StorageTrieUpdates>,
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

    /// Extends the trie updates.
    pub fn extend(&mut self, other: Self) {
        self.extend_common(&other);
        self.account_nodes.extend(exclude_empty_from_pair(other.account_nodes));
        self.removed_nodes.extend(exclude_empty(other.removed_nodes));
        for (hashed_address, storage_trie) in other.storage_tries {
            self.storage_tries.entry(hashed_address).or_default().extend(storage_trie);
        }
    }

    /// Extends the trie updates.
    ///
    /// Slightly less efficient than [`Self::extend`], but preferred to `extend(other.clone())`.
    pub fn extend_ref(&mut self, other: &Self) {
        self.extend_common(other);
        self.account_nodes.extend(exclude_empty_from_pair(
            other.account_nodes.iter().map(|(k, v)| (k.clone(), v.clone())),
        ));
        self.removed_nodes.extend(exclude_empty(other.removed_nodes.iter().cloned()));
        for (hashed_address, storage_trie) in &other.storage_tries {
            self.storage_tries.entry(*hashed_address).or_default().extend_ref(storage_trie);
        }
    }

    fn extend_common(&mut self, other: &Self) {
        self.account_nodes.retain(|nibbles, _| !other.removed_nodes.contains(nibbles));
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
        self.removed_nodes.extend(exclude_empty(removed_node_keys));

        // Retrieve updated nodes from hash builder.
        let (_, updated_nodes) = hash_builder.split();
        self.account_nodes.extend(exclude_empty_from_pair(updated_nodes));

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
}

/// Trie updates for storage trie of a single account.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StorageTrieUpdates {
    /// Flag indicating whether the trie was deleted.
    pub(crate) is_deleted: bool,
    /// Collection of updated storage trie nodes.
    #[cfg_attr(feature = "serde", serde(serialize_with = "serialize_nibbles_map"))]
    pub(crate) storage_nodes: HashMap<Nibbles, BranchNodeCompact>,
    /// Collection of removed storage trie nodes.
    pub(crate) removed_nodes: HashSet<Nibbles>,
}

#[cfg(feature = "test-utils")]
impl StorageTrieUpdates {
    /// Creates a new storage trie updates that are not marked as deleted.
    pub fn new(updates: impl IntoIterator<Item = (Nibbles, BranchNodeCompact)>) -> Self {
        Self { storage_nodes: exclude_empty_from_pair(updates).collect(), ..Default::default() }
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

    /// Extends storage trie updates.
    pub fn extend(&mut self, other: Self) {
        self.extend_common(&other);
        self.storage_nodes.extend(exclude_empty_from_pair(other.storage_nodes));
        self.removed_nodes.extend(exclude_empty(other.removed_nodes));
    }

    /// Extends storage trie updates.
    ///
    /// Slightly less efficient than [`Self::extend`], but preferred to `extend(other.clone())`.
    pub fn extend_ref(&mut self, other: &Self) {
        self.extend_common(other);
        self.storage_nodes.extend(exclude_empty_from_pair(
            other.storage_nodes.iter().map(|(k, v)| (k.clone(), v.clone())),
        ));
        self.removed_nodes.extend(exclude_empty(other.removed_nodes.iter().cloned()));
    }

    fn extend_common(&mut self, other: &Self) {
        if other.is_deleted {
            self.storage_nodes.clear();
            self.removed_nodes.clear();
        }
        self.is_deleted |= other.is_deleted;
        self.storage_nodes.retain(|nibbles, _| !other.removed_nodes.contains(nibbles));
    }

    /// Finalize storage trie updates for by taking updates from walker and hash builder.
    pub fn finalize<C>(&mut self, walker: TrieWalker<C>, hash_builder: HashBuilder) {
        // Retrieve deleted keys from trie walker.
        let (_, removed_keys) = walker.split();
        self.removed_nodes.extend(exclude_empty(removed_keys));

        // Retrieve updated nodes from hash builder.
        let (_, updated_nodes) = hash_builder.split();
        self.storage_nodes.extend(exclude_empty_from_pair(updated_nodes));
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
}

/// Serializes any [`HashMap`] that uses [`Nibbles`] as keys, by using the hex-encoded packed
/// representation.
///
/// This also sorts the map's keys before encoding and serializing.
#[cfg(feature = "serde")]
fn serialize_nibbles_map<S, T>(map: &HashMap<Nibbles, T>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize,
{
    let mut map_serializer = serializer.serialize_map(Some(map.len()))?;
    let mut storage_nodes = Vec::from_iter(map);
    storage_nodes.sort_unstable_by(|a, b| a.0.cmp(b.0));
    for (k, v) in storage_nodes {
        // pack, then hex encode the Nibbles
        let packed = reth_primitives::hex::encode(k.pack());
        map_serializer.serialize_entry(&packed, &v)?;
    }
    map_serializer.end()
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

/// Excludes empty nibbles from the given iterator.
fn exclude_empty(iter: impl IntoIterator<Item = Nibbles>) -> impl Iterator<Item = Nibbles> {
    iter.into_iter().filter(|n| !n.is_empty())
}

/// Excludes empty nibbles from the given iterator of pairs where the nibbles are the key.
fn exclude_empty_from_pair<V>(
    iter: impl IntoIterator<Item = (Nibbles, V)>,
) -> impl Iterator<Item = (Nibbles, V)> {
    iter.into_iter().filter(|(n, _)| !n.is_empty())
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_trie_updates_works() {
        let mut default_updates = TrieUpdates::default();
        let _updates_string = serde_json::to_string(&default_updates).unwrap();

        default_updates.removed_nodes.insert(Nibbles::from_vec(vec![0x0b, 0x0e, 0x0e, 0x0f]));
        let _updates_string = serde_json::to_string(&default_updates).unwrap();

        default_updates
            .account_nodes
            .insert(Nibbles::from_vec(vec![0x0b, 0x0e, 0x0f]), BranchNodeCompact::default());
        let _updates_string = serde_json::to_string(&default_updates).unwrap();

        default_updates.storage_tries.insert(B256::default(), StorageTrieUpdates::default());
        let _updates_string = serde_json::to_string(&default_updates).unwrap();
    }

    #[test]
    fn test_serialize_storage_trie_updates_works() {
        let mut default_updates = StorageTrieUpdates::default();
        let _updates_string = serde_json::to_string(&default_updates).unwrap();

        default_updates.removed_nodes.insert(Nibbles::from_vec(vec![0x0b, 0x0e, 0x0e, 0x0f]));
        let _updates_string = serde_json::to_string(&default_updates).unwrap();

        default_updates
            .storage_nodes
            .insert(Nibbles::from_vec(vec![0x0b, 0x0e, 0x0f]), BranchNodeCompact::default());
        let _updates_string = serde_json::to_string(&default_updates).unwrap();
    }
}
