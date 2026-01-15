use crate::{utils::extend_sorted_vec, BranchNodeCompact, HashBuilder, Nibbles};
use alloc::{
    collections::{btree_map::BTreeMap, btree_set::BTreeSet},
    vec::Vec,
};
use alloy_primitives::{
    map::{B256Map, B256Set, HashMap, HashSet},
    FixedBytes, B256,
};

/// The aggregation of trie updates.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
pub struct TrieUpdates {
    /// Collection of updated intermediate account nodes indexed by full path.
    #[cfg_attr(any(test, feature = "serde"), serde(with = "serde_nibbles_map"))]
    pub account_nodes: HashMap<Nibbles, BranchNodeCompact>,
    /// Collection of removed intermediate account nodes indexed by full path.
    #[cfg_attr(any(test, feature = "serde"), serde(with = "serde_nibbles_set"))]
    pub removed_nodes: HashSet<Nibbles>,
    /// Collection of updated storage tries indexed by the hashed address.
    pub storage_tries: B256Map<StorageTrieUpdates>,
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
    pub const fn storage_tries_ref(&self) -> &B256Map<StorageTrieUpdates> {
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
            other.account_nodes.iter().map(|(k, v)| (*k, v.clone())),
        ));
        self.removed_nodes.extend(exclude_empty(other.removed_nodes.iter().copied()));
        for (hashed_address, storage_trie) in &other.storage_tries {
            self.storage_tries.entry(*hashed_address).or_default().extend_ref(storage_trie);
        }
    }

    fn extend_common(&mut self, other: &Self) {
        self.account_nodes.retain(|nibbles, _| !other.removed_nodes.contains(nibbles));
    }

    /// Extend trie updates with sorted data, converting directly into the unsorted `HashMap`
    /// representation. This is more efficient than first converting to `TrieUpdates` and
    /// then extending, as it avoids creating intermediate `HashMap` allocations.
    ///
    /// This top-level helper merges account nodes and delegates each account's storage trie to
    /// [`StorageTrieUpdates::extend_from_sorted`].
    pub fn extend_from_sorted(&mut self, sorted: &TrieUpdatesSorted) {
        // Reserve capacity for account nodes
        let new_nodes_count = sorted.account_nodes.len();
        self.account_nodes.reserve(new_nodes_count);

        // Insert account nodes from sorted (only non-None entries)
        for (nibbles, maybe_node) in &sorted.account_nodes {
            if nibbles.is_empty() {
                continue;
            }
            match maybe_node {
                Some(node) => {
                    self.removed_nodes.remove(nibbles);
                    self.account_nodes.insert(*nibbles, node.clone());
                }
                None => {
                    self.account_nodes.remove(nibbles);
                    self.removed_nodes.insert(*nibbles);
                }
            }
        }

        // Extend storage tries
        self.storage_tries.reserve(sorted.storage_tries.len());
        for (hashed_address, sorted_storage) in &sorted.storage_tries {
            self.storage_tries
                .entry(*hashed_address)
                .or_default()
                .extend_from_sorted(sorted_storage);
        }
    }

    /// Insert storage updates for a given hashed address.
    pub fn insert_storage_updates(
        &mut self,
        hashed_address: B256,
        storage_updates: StorageTrieUpdates,
    ) {
        if storage_updates.is_empty() {
            return;
        }
        let existing = self.storage_tries.insert(hashed_address, storage_updates);
        debug_assert!(existing.is_none());
    }

    /// Finalize state trie updates.
    pub fn finalize(
        &mut self,
        hash_builder: HashBuilder,
        removed_keys: HashSet<Nibbles>,
        destroyed_accounts: B256Set,
    ) {
        // Retrieve updated nodes from hash builder.
        let (_, updated_nodes) = hash_builder.split();
        self.account_nodes.extend(exclude_empty_from_pair(updated_nodes));

        // Add deleted node paths.
        self.removed_nodes.extend(exclude_empty(removed_keys));

        // Add deleted storage tries for destroyed accounts.
        for destroyed in destroyed_accounts {
            self.storage_tries.entry(destroyed).or_default().set_deleted(true);
        }
    }

    /// Converts trie updates into [`TrieUpdatesSorted`].
    pub fn into_sorted(mut self) -> TrieUpdatesSorted {
        let mut account_nodes = self
            .account_nodes
            .drain()
            .map(|(path, node)| {
                // Updated nodes take precedence over removed nodes.
                self.removed_nodes.remove(&path);
                (path, Some(node))
            })
            .collect::<Vec<_>>();

        account_nodes.extend(self.removed_nodes.drain().map(|path| (path, None)));
        account_nodes.sort_unstable_by(|a, b| a.0.cmp(&b.0));

        let storage_tries = self
            .storage_tries
            .drain()
            .map(|(hashed_address, updates)| (hashed_address, updates.into_sorted()))
            .collect();
        TrieUpdatesSorted { account_nodes, storage_tries }
    }

    /// Creates a sorted copy without consuming self.
    /// More efficient than `.clone().into_sorted()` as it avoids cloning `HashMap` metadata.
    pub fn clone_into_sorted(&self) -> TrieUpdatesSorted {
        let mut account_nodes = self
            .account_nodes
            .iter()
            .map(|(path, node)| (*path, Some(node.clone())))
            .collect::<Vec<_>>();

        // Add removed nodes that aren't already updated (updated nodes take precedence)
        account_nodes.extend(
            self.removed_nodes
                .iter()
                .filter(|path| !self.account_nodes.contains_key(*path))
                .map(|path| (*path, None)),
        );
        account_nodes.sort_unstable_by(|a, b| a.0.cmp(&b.0));

        let storage_tries = self
            .storage_tries
            .iter()
            .map(|(&hashed_address, updates)| (hashed_address, updates.clone_into_sorted()))
            .collect();
        TrieUpdatesSorted { account_nodes, storage_tries }
    }

    /// Converts trie updates into [`TrieUpdatesSortedRef`].
    pub fn into_sorted_ref<'a>(&'a self) -> TrieUpdatesSortedRef<'a> {
        let mut account_nodes = self.account_nodes.iter().collect::<Vec<_>>();
        account_nodes.sort_unstable_by(|a, b| a.0.cmp(b.0));

        TrieUpdatesSortedRef {
            removed_nodes: self.removed_nodes.iter().collect::<BTreeSet<_>>(),
            account_nodes,
            storage_tries: self
                .storage_tries
                .iter()
                .map(|m| (*m.0, m.1.into_sorted_ref().clone()))
                .collect(),
        }
    }

    /// Clears the nodes and storage trie maps in this `TrieUpdates`.
    pub fn clear(&mut self) {
        self.account_nodes.clear();
        self.removed_nodes.clear();
        self.storage_tries.clear();
    }
}

/// Trie updates for storage trie of a single account.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
pub struct StorageTrieUpdates {
    /// Flag indicating whether the trie was deleted.
    pub is_deleted: bool,
    /// Collection of updated storage trie nodes.
    #[cfg_attr(any(test, feature = "serde"), serde(with = "serde_nibbles_map"))]
    pub storage_nodes: HashMap<Nibbles, BranchNodeCompact>,
    /// Collection of removed storage trie nodes.
    #[cfg_attr(any(test, feature = "serde"), serde(with = "serde_nibbles_set"))]
    pub removed_nodes: HashSet<Nibbles>,
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
    pub const fn set_deleted(&mut self, deleted: bool) {
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
            other.storage_nodes.iter().map(|(k, v)| (*k, v.clone())),
        ));
        self.removed_nodes.extend(exclude_empty(other.removed_nodes.iter().copied()));
    }

    fn extend_common(&mut self, other: &Self) {
        if other.is_deleted {
            self.storage_nodes.clear();
            self.removed_nodes.clear();
        }
        self.is_deleted |= other.is_deleted;
        self.storage_nodes.retain(|nibbles, _| !other.removed_nodes.contains(nibbles));
    }

    /// Extend storage trie updates with sorted data, converting directly into the unsorted
    /// `HashMap` representation. This is more efficient than first converting to
    /// `StorageTrieUpdates` and then extending, as it avoids creating intermediate `HashMap`
    /// allocations.
    ///
    /// This is invoked from [`TrieUpdates::extend_from_sorted`] for each account.
    pub fn extend_from_sorted(&mut self, sorted: &StorageTrieUpdatesSorted) {
        if sorted.is_deleted {
            self.storage_nodes.clear();
            self.removed_nodes.clear();
        }
        self.is_deleted |= sorted.is_deleted;

        // Reserve capacity for storage nodes
        let new_nodes_count = sorted.storage_nodes.len();
        self.storage_nodes.reserve(new_nodes_count);

        // Remove nodes marked as removed and insert new nodes
        for (nibbles, maybe_node) in &sorted.storage_nodes {
            if nibbles.is_empty() {
                continue;
            }
            if let Some(node) = maybe_node {
                self.removed_nodes.remove(nibbles);
                self.storage_nodes.insert(*nibbles, node.clone());
            } else {
                self.storage_nodes.remove(nibbles);
                self.removed_nodes.insert(*nibbles);
            }
        }
    }

    /// Finalize storage trie updates for by taking updates from walker and hash builder.
    pub fn finalize(&mut self, hash_builder: HashBuilder, removed_keys: HashSet<Nibbles>) {
        // Retrieve updated nodes from hash builder.
        let (_, updated_nodes) = hash_builder.split();
        self.storage_nodes.extend(exclude_empty_from_pair(updated_nodes));

        // Add deleted node paths.
        self.removed_nodes.extend(exclude_empty(removed_keys));
    }

    /// Convert storage trie updates into [`StorageTrieUpdatesSorted`].
    pub fn into_sorted(mut self) -> StorageTrieUpdatesSorted {
        let mut storage_nodes = self
            .storage_nodes
            .into_iter()
            .map(|(path, node)| {
                // Updated nodes take precedence over removed nodes.
                self.removed_nodes.remove(&path);
                (path, Some(node))
            })
            .collect::<Vec<_>>();

        storage_nodes.extend(self.removed_nodes.into_iter().map(|path| (path, None)));
        storage_nodes.sort_unstable_by(|a, b| a.0.cmp(&b.0));

        StorageTrieUpdatesSorted { is_deleted: self.is_deleted, storage_nodes }
    }

    /// Creates a sorted copy without consuming self.
    /// More efficient than `.clone().into_sorted()` as it avoids cloning `HashMap` metadata.
    pub fn clone_into_sorted(&self) -> StorageTrieUpdatesSorted {
        let mut storage_nodes = self
            .storage_nodes
            .iter()
            .map(|(path, node)| (*path, Some(node.clone())))
            .collect::<Vec<_>>();

        // Add removed nodes that aren't already updated (updated nodes take precedence)
        storage_nodes.extend(
            self.removed_nodes
                .iter()
                .filter(|path| !self.storage_nodes.contains_key(*path))
                .map(|path| (*path, None)),
        );
        storage_nodes.sort_unstable_by(|a, b| a.0.cmp(&b.0));

        StorageTrieUpdatesSorted { is_deleted: self.is_deleted, storage_nodes }
    }

    /// Convert storage trie updates into [`StorageTrieUpdatesSortedRef`].
    pub fn into_sorted_ref(&self) -> StorageTrieUpdatesSortedRef<'_> {
        StorageTrieUpdatesSortedRef {
            is_deleted: self.is_deleted,
            removed_nodes: self.removed_nodes.iter().collect::<BTreeSet<_>>(),
            storage_nodes: self.storage_nodes.iter().collect::<BTreeMap<_, _>>(),
        }
    }
}

/// Serializes and deserializes any [`HashSet`] that includes [`Nibbles`] elements, by using the
/// hex-encoded packed representation.
///
/// This also sorts the set before serializing.
#[cfg(any(test, feature = "serde"))]
mod serde_nibbles_set {
    use crate::Nibbles;
    use alloc::{
        string::{String, ToString},
        vec::Vec,
    };
    use alloy_primitives::map::HashSet;
    use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S>(map: &HashSet<Nibbles>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut storage_nodes =
            map.iter().map(|elem| alloy_primitives::hex::encode(elem.pack())).collect::<Vec<_>>();
        storage_nodes.sort_unstable();
        storage_nodes.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<HashSet<Nibbles>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<String>::deserialize(deserializer)?
            .into_iter()
            .map(|node| {
                Ok(Nibbles::unpack(
                    alloy_primitives::hex::decode(node)
                        .map_err(|err| D::Error::custom(err.to_string()))?,
                ))
            })
            .collect::<Result<HashSet<_>, _>>()
    }
}

/// Serializes and deserializes any [`HashMap`] that uses [`Nibbles`] as keys, by using the
/// hex-encoded packed representation.
///
/// This also sorts the map's keys before encoding and serializing.
#[cfg(any(test, feature = "serde"))]
mod serde_nibbles_map {
    use crate::Nibbles;
    use alloc::{
        string::{String, ToString},
        vec::Vec,
    };
    use alloy_primitives::{hex, map::HashMap};
    use core::marker::PhantomData;
    use serde::{
        de::{Error, MapAccess, Visitor},
        ser::SerializeMap,
        Deserialize, Deserializer, Serialize, Serializer,
    };

    pub(super) fn serialize<S, T>(
        map: &HashMap<Nibbles, T>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: Serialize,
    {
        let mut map_serializer = serializer.serialize_map(Some(map.len()))?;
        let mut storage_nodes = Vec::from_iter(map);
        storage_nodes.sort_unstable_by_key(|node| node.0);
        for (k, v) in storage_nodes {
            // pack, then hex encode the Nibbles
            let packed = alloy_primitives::hex::encode(k.pack());
            map_serializer.serialize_entry(&packed, &v)?;
        }
        map_serializer.end()
    }

    pub(super) fn deserialize<'de, D, T>(deserializer: D) -> Result<HashMap<Nibbles, T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        struct NibblesMapVisitor<T> {
            marker: PhantomData<T>,
        }

        impl<'de, T> Visitor<'de> for NibblesMapVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = HashMap<Nibbles, T>;

            fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter.write_str("a map with hex-encoded Nibbles keys")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut result = HashMap::with_capacity_and_hasher(
                    map.size_hint().unwrap_or(0),
                    Default::default(),
                );

                while let Some((key, value)) = map.next_entry::<String, T>()? {
                    let decoded_key =
                        hex::decode(&key).map_err(|err| Error::custom(err.to_string()))?;

                    let nibbles = Nibbles::unpack(&decoded_key);

                    result.insert(nibbles, value);
                }

                Ok(result)
            }
        }

        deserializer.deserialize_map(NibblesMapVisitor { marker: PhantomData })
    }
}

/// Sorted trie updates reference used for serializing trie to file.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize))]
pub struct TrieUpdatesSortedRef<'a> {
    /// Sorted collection of updated state nodes with corresponding paths.
    pub account_nodes: Vec<(&'a Nibbles, &'a BranchNodeCompact)>,
    /// The set of removed state node keys.
    pub removed_nodes: BTreeSet<&'a Nibbles>,
    /// Storage tries stored by hashed address of the account the trie belongs to.
    pub storage_tries: BTreeMap<FixedBytes<32>, StorageTrieUpdatesSortedRef<'a>>,
}

/// Sorted trie updates used for lookups and insertions.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
pub struct TrieUpdatesSorted {
    /// Sorted collection of updated state nodes with corresponding paths. None indicates that a
    /// node was removed.
    account_nodes: Vec<(Nibbles, Option<BranchNodeCompact>)>,
    /// Storage tries stored by hashed address of the account the trie belongs to.
    storage_tries: B256Map<StorageTrieUpdatesSorted>,
}

impl TrieUpdatesSorted {
    /// Creates a new `TrieUpdatesSorted` with the given account nodes and storage tries.
    ///
    /// # Panics
    ///
    /// In debug mode, panics if `account_nodes` is not sorted by the `Nibbles` key,
    /// or if any storage trie's `storage_nodes` is not sorted by its `Nibbles` key.
    pub fn new(
        account_nodes: Vec<(Nibbles, Option<BranchNodeCompact>)>,
        storage_tries: B256Map<StorageTrieUpdatesSorted>,
    ) -> Self {
        debug_assert!(
            account_nodes.is_sorted_by_key(|item| &item.0),
            "account_nodes must be sorted by Nibbles key"
        );
        debug_assert!(
            storage_tries.values().all(|storage_trie| {
                storage_trie.storage_nodes.is_sorted_by_key(|item| &item.0)
            }),
            "all storage_nodes in storage_tries must be sorted by Nibbles key"
        );
        Self { account_nodes, storage_tries }
    }

    /// Returns `true` if the updates are empty.
    pub fn is_empty(&self) -> bool {
        self.account_nodes.is_empty() && self.storage_tries.is_empty()
    }

    /// Returns reference to updated account nodes.
    pub fn account_nodes_ref(&self) -> &[(Nibbles, Option<BranchNodeCompact>)] {
        &self.account_nodes
    }

    /// Returns reference to updated storage tries.
    pub const fn storage_tries_ref(&self) -> &B256Map<StorageTrieUpdatesSorted> {
        &self.storage_tries
    }

    /// Returns the total number of updates including account nodes and all storage updates.
    pub fn total_len(&self) -> usize {
        self.account_nodes.len() +
            self.storage_tries.values().map(|storage| storage.len()).sum::<usize>()
    }

    /// Extends the trie updates with another set of sorted updates.
    ///
    /// This merges the account nodes and storage tries from `other` into `self`.
    /// Account nodes are merged and re-sorted, with `other`'s values taking precedence
    /// for duplicate keys.
    ///
    /// Sorts the account nodes after extending. Sorts the storage tries after extending, for each
    /// storage trie.
    pub fn extend_ref_and_sort(&mut self, other: &Self) {
        // Extend account nodes
        extend_sorted_vec(&mut self.account_nodes, &other.account_nodes);

        // Merge storage tries
        for (hashed_address, storage_trie) in &other.storage_tries {
            self.storage_tries
                .entry(*hashed_address)
                .and_modify(|existing| existing.extend_ref(storage_trie))
                .or_insert_with(|| storage_trie.clone());
        }
    }

    /// Clears all account nodes and storage tries.
    pub fn clear(&mut self) {
        self.account_nodes.clear();
        self.storage_tries.clear();
    }
}

impl AsRef<Self> for TrieUpdatesSorted {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl From<TrieUpdatesSorted> for TrieUpdates {
    fn from(sorted: TrieUpdatesSorted) -> Self {
        let mut account_nodes = HashMap::default();
        let mut removed_nodes = HashSet::default();

        for (nibbles, node) in sorted.account_nodes {
            if let Some(node) = node {
                account_nodes.insert(nibbles, node);
            } else {
                removed_nodes.insert(nibbles);
            }
        }

        let storage_tries = sorted
            .storage_tries
            .into_iter()
            .map(|(address, storage)| (address, storage.into()))
            .collect();

        Self { account_nodes, removed_nodes, storage_tries }
    }
}

/// Sorted storage trie updates reference used for serializing to file.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize))]
pub struct StorageTrieUpdatesSortedRef<'a> {
    /// Flag indicating whether the trie has been deleted/wiped.
    pub is_deleted: bool,
    /// Sorted collection of updated storage nodes with corresponding paths.
    pub storage_nodes: BTreeMap<&'a Nibbles, &'a BranchNodeCompact>,
    /// The set of removed storage node keys.
    pub removed_nodes: BTreeSet<&'a Nibbles>,
}

/// Sorted trie updates used for lookups and insertions.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
pub struct StorageTrieUpdatesSorted {
    /// Flag indicating whether the trie has been deleted/wiped.
    pub is_deleted: bool,
    /// Sorted collection of updated storage nodes with corresponding paths. None indicates a node
    /// is removed.
    pub storage_nodes: Vec<(Nibbles, Option<BranchNodeCompact>)>,
}

impl StorageTrieUpdatesSorted {
    /// Returns `true` if the trie was deleted.
    pub const fn is_deleted(&self) -> bool {
        self.is_deleted
    }

    /// Returns reference to updated storage nodes.
    pub fn storage_nodes_ref(&self) -> &[(Nibbles, Option<BranchNodeCompact>)] {
        &self.storage_nodes
    }

    /// Returns the total number of storage node updates.
    pub const fn len(&self) -> usize {
        self.storage_nodes.len()
    }

    /// Returns `true` if there are no storage node updates.
    pub const fn is_empty(&self) -> bool {
        self.storage_nodes.is_empty()
    }

    /// Extends the storage trie updates with another set of sorted updates.
    ///
    /// If `other` is marked as deleted, this will be marked as deleted and all nodes cleared.
    /// Otherwise, nodes are merged with `other`'s values taking precedence for duplicates.
    pub fn extend_ref(&mut self, other: &Self) {
        if other.is_deleted {
            self.is_deleted = true;
            self.storage_nodes.clear();
            self.storage_nodes.extend(other.storage_nodes.iter().cloned());
            return;
        }

        // Extend storage nodes
        extend_sorted_vec(&mut self.storage_nodes, &other.storage_nodes);
        self.is_deleted = self.is_deleted || other.is_deleted;
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

impl From<StorageTrieUpdatesSorted> for StorageTrieUpdates {
    fn from(sorted: StorageTrieUpdatesSorted) -> Self {
        let mut storage_nodes = HashMap::default();
        let mut removed_nodes = HashSet::default();

        for (nibbles, node) in sorted.storage_nodes {
            if let Some(node) = node {
                storage_nodes.insert(nibbles, node);
            } else {
                removed_nodes.insert(nibbles);
            }
        }

        Self { is_deleted: sorted.is_deleted, storage_nodes, removed_nodes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn test_trie_updates_sorted_extend_ref() {
        // Test extending with empty updates
        let mut updates1 = TrieUpdatesSorted::default();
        let updates2 = TrieUpdatesSorted::default();
        updates1.extend_ref_and_sort(&updates2);
        assert_eq!(updates1.account_nodes.len(), 0);
        assert_eq!(updates1.storage_tries.len(), 0);

        // Test extending account nodes
        let mut updates1 = TrieUpdatesSorted {
            account_nodes: vec![
                (Nibbles::from_nibbles_unchecked([0x01]), Some(BranchNodeCompact::default())),
                (Nibbles::from_nibbles_unchecked([0x03]), None),
            ],
            storage_tries: B256Map::default(),
        };
        let updates2 = TrieUpdatesSorted {
            account_nodes: vec![
                (Nibbles::from_nibbles_unchecked([0x02]), Some(BranchNodeCompact::default())),
                (Nibbles::from_nibbles_unchecked([0x03]), Some(BranchNodeCompact::default())), /* Override */
            ],
            storage_tries: B256Map::default(),
        };
        updates1.extend_ref_and_sort(&updates2);
        assert_eq!(updates1.account_nodes.len(), 3);
        // Should be sorted: 0x01, 0x02, 0x03
        assert_eq!(updates1.account_nodes[0].0, Nibbles::from_nibbles_unchecked([0x01]));
        assert_eq!(updates1.account_nodes[1].0, Nibbles::from_nibbles_unchecked([0x02]));
        assert_eq!(updates1.account_nodes[2].0, Nibbles::from_nibbles_unchecked([0x03]));
        // 0x03 should have Some value from updates2 (override)
        assert!(updates1.account_nodes[2].1.is_some());

        // Test extending storage tries
        let storage_trie1 = StorageTrieUpdatesSorted {
            is_deleted: false,
            storage_nodes: vec![(
                Nibbles::from_nibbles_unchecked([0x0a]),
                Some(BranchNodeCompact::default()),
            )],
        };
        let storage_trie2 = StorageTrieUpdatesSorted {
            is_deleted: false,
            storage_nodes: vec![(Nibbles::from_nibbles_unchecked([0x0b]), None)],
        };

        let hashed_address1 = B256::from([1; 32]);
        let hashed_address2 = B256::from([2; 32]);

        let mut updates1 = TrieUpdatesSorted {
            account_nodes: vec![],
            storage_tries: B256Map::from_iter([(hashed_address1, storage_trie1.clone())]),
        };
        let updates2 = TrieUpdatesSorted {
            account_nodes: vec![],
            storage_tries: B256Map::from_iter([
                (hashed_address1, storage_trie2),
                (hashed_address2, storage_trie1),
            ]),
        };
        updates1.extend_ref_and_sort(&updates2);
        assert_eq!(updates1.storage_tries.len(), 2);
        assert!(updates1.storage_tries.contains_key(&hashed_address1));
        assert!(updates1.storage_tries.contains_key(&hashed_address2));
        // Check that storage trie for hashed_address1 was extended
        let merged_storage = &updates1.storage_tries[&hashed_address1];
        assert_eq!(merged_storage.storage_nodes.len(), 2);
    }

    #[test]
    fn test_storage_trie_updates_sorted_extend_ref_deleted() {
        // Test case 1: Extending with a deleted storage trie that has nodes
        let mut storage1 = StorageTrieUpdatesSorted {
            is_deleted: false,
            storage_nodes: vec![
                (Nibbles::from_nibbles_unchecked([0x01]), Some(BranchNodeCompact::default())),
                (Nibbles::from_nibbles_unchecked([0x02]), None),
            ],
        };

        let storage2 = StorageTrieUpdatesSorted {
            is_deleted: true,
            storage_nodes: vec![
                (Nibbles::from_nibbles_unchecked([0x03]), Some(BranchNodeCompact::default())),
                (Nibbles::from_nibbles_unchecked([0x04]), None),
            ],
        };

        storage1.extend_ref(&storage2);

        // Should be marked as deleted
        assert!(storage1.is_deleted);
        // Original nodes should be cleared, but other's nodes should be added
        assert_eq!(storage1.storage_nodes.len(), 2);
        assert_eq!(storage1.storage_nodes[0].0, Nibbles::from_nibbles_unchecked([0x03]));
        assert_eq!(storage1.storage_nodes[1].0, Nibbles::from_nibbles_unchecked([0x04]));

        // Test case 2: Extending a deleted storage trie with more nodes
        let mut storage3 = StorageTrieUpdatesSorted {
            is_deleted: true,
            storage_nodes: vec![(
                Nibbles::from_nibbles_unchecked([0x05]),
                Some(BranchNodeCompact::default()),
            )],
        };

        let storage4 = StorageTrieUpdatesSorted {
            is_deleted: true,
            storage_nodes: vec![
                (Nibbles::from_nibbles_unchecked([0x06]), Some(BranchNodeCompact::default())),
                (Nibbles::from_nibbles_unchecked([0x07]), None),
            ],
        };

        storage3.extend_ref(&storage4);

        // Should remain deleted
        assert!(storage3.is_deleted);
        // Should have nodes from other (original cleared then extended)
        assert_eq!(storage3.storage_nodes.len(), 2);
        assert_eq!(storage3.storage_nodes[0].0, Nibbles::from_nibbles_unchecked([0x06]));
        assert_eq!(storage3.storage_nodes[1].0, Nibbles::from_nibbles_unchecked([0x07]));
    }

    /// Test extending with storage tries adds both nodes and removed nodes correctly
    #[test]
    fn test_trie_updates_extend_from_sorted_with_storage_tries() {
        let hashed_address = B256::from([1; 32]);

        let mut updates = TrieUpdates::default();

        let storage_trie = StorageTrieUpdatesSorted {
            is_deleted: false,
            storage_nodes: vec![
                (Nibbles::from_nibbles_unchecked([0x0a]), Some(BranchNodeCompact::default())),
                (Nibbles::from_nibbles_unchecked([0x0b]), None),
            ],
        };

        let sorted = TrieUpdatesSorted {
            account_nodes: vec![],
            storage_tries: B256Map::from_iter([(hashed_address, storage_trie)]),
        };

        updates.extend_from_sorted(&sorted);

        assert_eq!(updates.storage_tries.len(), 1);
        let storage = updates.storage_tries.get(&hashed_address).unwrap();
        assert!(!storage.is_deleted);
        assert_eq!(storage.storage_nodes.len(), 1);
        assert!(storage.removed_nodes.contains(&Nibbles::from_nibbles_unchecked([0x0b])));
    }

    /// Test deleted=true clears old storage nodes before adding new ones (critical edge case)
    #[test]
    fn test_trie_updates_extend_from_sorted_with_deleted_storage() {
        let hashed_address = B256::from([1; 32]);

        let mut updates = TrieUpdates::default();
        updates.storage_tries.insert(
            hashed_address,
            StorageTrieUpdates {
                is_deleted: false,
                storage_nodes: HashMap::from_iter([(
                    Nibbles::from_nibbles_unchecked([0x01]),
                    BranchNodeCompact::default(),
                )]),
                removed_nodes: Default::default(),
            },
        );

        let storage_trie = StorageTrieUpdatesSorted {
            is_deleted: true,
            storage_nodes: vec![(
                Nibbles::from_nibbles_unchecked([0x0a]),
                Some(BranchNodeCompact::default()),
            )],
        };

        let sorted = TrieUpdatesSorted {
            account_nodes: vec![],
            storage_tries: B256Map::from_iter([(hashed_address, storage_trie)]),
        };

        updates.extend_from_sorted(&sorted);

        let storage = updates.storage_tries.get(&hashed_address).unwrap();
        assert!(storage.is_deleted);
        // After deletion, old nodes should be cleared
        assert_eq!(storage.storage_nodes.len(), 1);
        assert!(storage.storage_nodes.contains_key(&Nibbles::from_nibbles_unchecked([0x0a])));
    }

    /// Test non-deleted storage merges nodes and tracks removed nodes
    #[test]
    fn test_storage_trie_updates_extend_from_sorted_non_deleted() {
        let mut storage = StorageTrieUpdates {
            is_deleted: false,
            storage_nodes: HashMap::from_iter([(
                Nibbles::from_nibbles_unchecked([0x01]),
                BranchNodeCompact::default(),
            )]),
            removed_nodes: Default::default(),
        };

        let sorted = StorageTrieUpdatesSorted {
            is_deleted: false,
            storage_nodes: vec![
                (Nibbles::from_nibbles_unchecked([0x02]), Some(BranchNodeCompact::default())),
                (Nibbles::from_nibbles_unchecked([0x03]), None),
            ],
        };

        storage.extend_from_sorted(&sorted);

        assert!(!storage.is_deleted);
        assert_eq!(storage.storage_nodes.len(), 2);
        assert!(storage.removed_nodes.contains(&Nibbles::from_nibbles_unchecked([0x03])));
    }

    /// Test deleted=true clears old nodes before extending (edge case)
    #[test]
    fn test_storage_trie_updates_extend_from_sorted_deleted() {
        let mut storage = StorageTrieUpdates {
            is_deleted: false,
            storage_nodes: HashMap::from_iter([(
                Nibbles::from_nibbles_unchecked([0x01]),
                BranchNodeCompact::default(),
            )]),
            removed_nodes: Default::default(),
        };

        let sorted = StorageTrieUpdatesSorted {
            is_deleted: true,
            storage_nodes: vec![(
                Nibbles::from_nibbles_unchecked([0x0a]),
                Some(BranchNodeCompact::default()),
            )],
        };

        storage.extend_from_sorted(&sorted);

        assert!(storage.is_deleted);
        // Old nodes should be cleared when deleted
        assert_eq!(storage.storage_nodes.len(), 1);
        assert!(storage.storage_nodes.contains_key(&Nibbles::from_nibbles_unchecked([0x0a])));
    }

    /// Test empty nibbles are filtered out during conversion (edge case bug)
    #[test]
    fn test_trie_updates_extend_from_sorted_filters_empty_nibbles() {
        let mut updates = TrieUpdates::default();

        let sorted = TrieUpdatesSorted {
            account_nodes: vec![
                (Nibbles::default(), Some(BranchNodeCompact::default())), // Empty nibbles
                (Nibbles::from_nibbles_unchecked([0x01]), Some(BranchNodeCompact::default())),
            ],
            storage_tries: B256Map::default(),
        };

        updates.extend_from_sorted(&sorted);

        // Empty nibbles should be filtered out
        assert_eq!(updates.account_nodes.len(), 1);
        assert!(updates.account_nodes.contains_key(&Nibbles::from_nibbles_unchecked([0x01])));
        assert!(!updates.account_nodes.contains_key(&Nibbles::default()));
    }
}

/// Bincode-compatible trie updates type serde implementations.
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    use crate::{BranchNodeCompact, Nibbles};
    use alloc::borrow::Cow;
    use alloy_primitives::map::{B256Map, HashMap, HashSet};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::TrieUpdates`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_trie_common::{serde_bincode_compat, updates::TrieUpdates};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::updates::TrieUpdates")]
    ///     trie_updates: TrieUpdates,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct TrieUpdates<'a> {
        account_nodes: Cow<'a, HashMap<Nibbles, BranchNodeCompact>>,
        removed_nodes: Cow<'a, HashSet<Nibbles>>,
        storage_tries: B256Map<StorageTrieUpdates<'a>>,
    }

    impl<'a> From<&'a super::TrieUpdates> for TrieUpdates<'a> {
        fn from(value: &'a super::TrieUpdates) -> Self {
            Self {
                account_nodes: Cow::Borrowed(&value.account_nodes),
                removed_nodes: Cow::Borrowed(&value.removed_nodes),
                storage_tries: value.storage_tries.iter().map(|(k, v)| (*k, v.into())).collect(),
            }
        }
    }

    impl<'a> From<TrieUpdates<'a>> for super::TrieUpdates {
        fn from(value: TrieUpdates<'a>) -> Self {
            Self {
                account_nodes: value.account_nodes.into_owned(),
                removed_nodes: value.removed_nodes.into_owned(),
                storage_tries: value
                    .storage_tries
                    .into_iter()
                    .map(|(k, v)| (k, v.into()))
                    .collect(),
            }
        }
    }

    impl SerializeAs<super::TrieUpdates> for TrieUpdates<'_> {
        fn serialize_as<S>(source: &super::TrieUpdates, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            TrieUpdates::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::TrieUpdates> for TrieUpdates<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::TrieUpdates, D::Error>
        where
            D: Deserializer<'de>,
        {
            TrieUpdates::deserialize(deserializer).map(Into::into)
        }
    }

    /// Bincode-compatible [`super::StorageTrieUpdates`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_trie_common::{serde_bincode_compat, updates::StorageTrieUpdates};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::updates::StorageTrieUpdates")]
    ///     trie_updates: StorageTrieUpdates,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct StorageTrieUpdates<'a> {
        is_deleted: bool,
        storage_nodes: Cow<'a, HashMap<Nibbles, BranchNodeCompact>>,
        removed_nodes: Cow<'a, HashSet<Nibbles>>,
    }

    impl<'a> From<&'a super::StorageTrieUpdates> for StorageTrieUpdates<'a> {
        fn from(value: &'a super::StorageTrieUpdates) -> Self {
            Self {
                is_deleted: value.is_deleted,
                storage_nodes: Cow::Borrowed(&value.storage_nodes),
                removed_nodes: Cow::Borrowed(&value.removed_nodes),
            }
        }
    }

    impl<'a> From<StorageTrieUpdates<'a>> for super::StorageTrieUpdates {
        fn from(value: StorageTrieUpdates<'a>) -> Self {
            Self {
                is_deleted: value.is_deleted,
                storage_nodes: value.storage_nodes.into_owned(),
                removed_nodes: value.removed_nodes.into_owned(),
            }
        }
    }

    impl SerializeAs<super::StorageTrieUpdates> for StorageTrieUpdates<'_> {
        fn serialize_as<S>(
            source: &super::StorageTrieUpdates,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            StorageTrieUpdates::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::StorageTrieUpdates> for StorageTrieUpdates<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::StorageTrieUpdates, D::Error>
        where
            D: Deserializer<'de>,
        {
            StorageTrieUpdates::deserialize(deserializer).map(Into::into)
        }
    }

    /// Bincode-compatible [`super::TrieUpdatesSorted`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_trie_common::{serde_bincode_compat, updates::TrieUpdatesSorted};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::updates::TrieUpdatesSorted")]
    ///     trie_updates: TrieUpdatesSorted,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct TrieUpdatesSorted<'a> {
        account_nodes: Cow<'a, [(Nibbles, Option<BranchNodeCompact>)]>,
        storage_tries: B256Map<StorageTrieUpdatesSorted<'a>>,
    }

    impl<'a> From<&'a super::TrieUpdatesSorted> for TrieUpdatesSorted<'a> {
        fn from(value: &'a super::TrieUpdatesSorted) -> Self {
            Self {
                account_nodes: Cow::Borrowed(&value.account_nodes),
                storage_tries: value.storage_tries.iter().map(|(k, v)| (*k, v.into())).collect(),
            }
        }
    }

    impl<'a> From<TrieUpdatesSorted<'a>> for super::TrieUpdatesSorted {
        fn from(value: TrieUpdatesSorted<'a>) -> Self {
            Self {
                account_nodes: value.account_nodes.into_owned(),
                storage_tries: value
                    .storage_tries
                    .into_iter()
                    .map(|(k, v)| (k, v.into()))
                    .collect(),
            }
        }
    }

    impl SerializeAs<super::TrieUpdatesSorted> for TrieUpdatesSorted<'_> {
        fn serialize_as<S>(
            source: &super::TrieUpdatesSorted,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            TrieUpdatesSorted::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::TrieUpdatesSorted> for TrieUpdatesSorted<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::TrieUpdatesSorted, D::Error>
        where
            D: Deserializer<'de>,
        {
            TrieUpdatesSorted::deserialize(deserializer).map(Into::into)
        }
    }

    /// Bincode-compatible [`super::StorageTrieUpdatesSorted`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_trie_common::{serde_bincode_compat, updates::StorageTrieUpdatesSorted};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::updates::StorageTrieUpdatesSorted")]
    ///     trie_updates: StorageTrieUpdatesSorted,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct StorageTrieUpdatesSorted<'a> {
        is_deleted: bool,
        storage_nodes: Cow<'a, [(Nibbles, Option<BranchNodeCompact>)]>,
    }

    impl<'a> From<&'a super::StorageTrieUpdatesSorted> for StorageTrieUpdatesSorted<'a> {
        fn from(value: &'a super::StorageTrieUpdatesSorted) -> Self {
            Self {
                is_deleted: value.is_deleted,
                storage_nodes: Cow::Borrowed(&value.storage_nodes),
            }
        }
    }

    impl<'a> From<StorageTrieUpdatesSorted<'a>> for super::StorageTrieUpdatesSorted {
        fn from(value: StorageTrieUpdatesSorted<'a>) -> Self {
            Self { is_deleted: value.is_deleted, storage_nodes: value.storage_nodes.into_owned() }
        }
    }

    impl SerializeAs<super::StorageTrieUpdatesSorted> for StorageTrieUpdatesSorted<'_> {
        fn serialize_as<S>(
            source: &super::StorageTrieUpdatesSorted,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            StorageTrieUpdatesSorted::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::StorageTrieUpdatesSorted> for StorageTrieUpdatesSorted<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::StorageTrieUpdatesSorted, D::Error>
        where
            D: Deserializer<'de>,
        {
            StorageTrieUpdatesSorted::deserialize(deserializer).map(Into::into)
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::{
            serde_bincode_compat,
            updates::{
                StorageTrieUpdates, StorageTrieUpdatesSorted, TrieUpdates, TrieUpdatesSorted,
            },
            BranchNodeCompact, Nibbles,
        };
        use alloy_primitives::B256;
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        #[test]
        fn test_trie_updates_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::updates::TrieUpdates")]
                trie_updates: TrieUpdates,
            }

            let mut data = Data { trie_updates: TrieUpdates::default() };
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates
                .removed_nodes
                .insert(Nibbles::from_nibbles_unchecked([0x0b, 0x0e, 0x0e, 0x0f]));
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates.account_nodes.insert(
                Nibbles::from_nibbles_unchecked([0x0d, 0x0e, 0x0a, 0x0d]),
                BranchNodeCompact::default(),
            );
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates.storage_tries.insert(B256::default(), StorageTrieUpdates::default());
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }

        #[test]
        fn test_storage_trie_updates_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::updates::StorageTrieUpdates")]
                trie_updates: StorageTrieUpdates,
            }

            let mut data = Data { trie_updates: StorageTrieUpdates::default() };
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates
                .removed_nodes
                .insert(Nibbles::from_nibbles_unchecked([0x0b, 0x0e, 0x0e, 0x0f]));
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates.storage_nodes.insert(
                Nibbles::from_nibbles_unchecked([0x0d, 0x0e, 0x0a, 0x0d]),
                BranchNodeCompact::default(),
            );
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }

        #[test]
        fn test_trie_updates_sorted_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::updates::TrieUpdatesSorted")]
                trie_updates: TrieUpdatesSorted,
            }

            let mut data = Data { trie_updates: TrieUpdatesSorted::default() };
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates.account_nodes.push((
                Nibbles::from_nibbles_unchecked([0x0d, 0x0e, 0x0a, 0x0d]),
                Some(BranchNodeCompact::default()),
            ));
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates
                .account_nodes
                .push((Nibbles::from_nibbles_unchecked([0x0f, 0x0f, 0x0f, 0x0f]), None));
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates
                .storage_tries
                .insert(B256::default(), StorageTrieUpdatesSorted::default());
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }

        #[test]
        fn test_storage_trie_updates_sorted_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::updates::StorageTrieUpdatesSorted")]
                trie_updates: StorageTrieUpdatesSorted,
            }

            let mut data = Data { trie_updates: StorageTrieUpdatesSorted::default() };
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates.storage_nodes.push((
                Nibbles::from_nibbles_unchecked([0x0d, 0x0e, 0x0a, 0x0d]),
                Some(BranchNodeCompact::default()),
            ));
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates
                .storage_nodes
                .push((Nibbles::from_nibbles_unchecked([0x0a, 0x0a, 0x0a, 0x0a]), None));
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates.is_deleted = true;
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }
    }
}

#[cfg(all(test, feature = "serde"))]
mod serde_tests {
    use super::*;

    #[test]
    fn test_trie_updates_serde_roundtrip() {
        let mut default_updates = TrieUpdates::default();
        let updates_serialized = serde_json::to_string(&default_updates).unwrap();
        let updates_deserialized: TrieUpdates = serde_json::from_str(&updates_serialized).unwrap();
        assert_eq!(updates_deserialized, default_updates);

        default_updates
            .removed_nodes
            .insert(Nibbles::from_nibbles_unchecked([0x0b, 0x0e, 0x0e, 0x0f]));
        let updates_serialized = serde_json::to_string(&default_updates).unwrap();
        let updates_deserialized: TrieUpdates = serde_json::from_str(&updates_serialized).unwrap();
        assert_eq!(updates_deserialized, default_updates);

        default_updates.account_nodes.insert(
            Nibbles::from_nibbles_unchecked([0x0d, 0x0e, 0x0a, 0x0d]),
            BranchNodeCompact::default(),
        );
        let updates_serialized = serde_json::to_string(&default_updates).unwrap();
        let updates_deserialized: TrieUpdates = serde_json::from_str(&updates_serialized).unwrap();
        assert_eq!(updates_deserialized, default_updates);

        default_updates.storage_tries.insert(B256::default(), StorageTrieUpdates::default());
        let updates_serialized = serde_json::to_string(&default_updates).unwrap();
        let updates_deserialized: TrieUpdates = serde_json::from_str(&updates_serialized).unwrap();
        assert_eq!(updates_deserialized, default_updates);
    }

    #[test]
    fn test_storage_trie_updates_serde_roundtrip() {
        let mut default_updates = StorageTrieUpdates::default();
        let updates_serialized = serde_json::to_string(&default_updates).unwrap();
        let updates_deserialized: StorageTrieUpdates =
            serde_json::from_str(&updates_serialized).unwrap();
        assert_eq!(updates_deserialized, default_updates);

        default_updates
            .removed_nodes
            .insert(Nibbles::from_nibbles_unchecked([0x0b, 0x0e, 0x0e, 0x0f]));
        let updates_serialized = serde_json::to_string(&default_updates).unwrap();
        let updates_deserialized: StorageTrieUpdates =
            serde_json::from_str(&updates_serialized).unwrap();
        assert_eq!(updates_deserialized, default_updates);

        default_updates.storage_nodes.insert(
            Nibbles::from_nibbles_unchecked([0x0d, 0x0e, 0x0a, 0x0d]),
            BranchNodeCompact::default(),
        );
        let updates_serialized = serde_json::to_string(&default_updates).unwrap();
        let updates_deserialized: StorageTrieUpdates =
            serde_json::from_str(&updates_serialized).unwrap();
        assert_eq!(updates_deserialized, default_updates);
    }
}
