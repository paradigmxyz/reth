use crate::{walker::TrieWalker, BranchNodeCompact, HashBuilder, Nibbles};
use alloy_primitives::B256;
use std::collections::{HashMap, HashSet};

/// The aggregation of trie updates.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrieUpdates {
    #[cfg_attr(feature = "serde", serde(with = "serde_nibbles_map"))]
    pub(crate) account_nodes: HashMap<Nibbles, BranchNodeCompact>,
    #[cfg_attr(feature = "serde", serde(with = "serde_nibbles_set"))]
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
    #[cfg_attr(feature = "serde", serde(with = "serde_nibbles_map"))]
    pub(crate) storage_nodes: HashMap<Nibbles, BranchNodeCompact>,
    /// Collection of removed storage trie nodes.
    #[cfg_attr(feature = "serde", serde(with = "serde_nibbles_set"))]
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

/// Serializes and deserializes any [`HashSet`] that includes [`Nibbles`] elements, by using the
/// hex-encoded packed representation.
///
/// This also sorts the set before serializing.
#[cfg(feature = "serde")]
mod serde_nibbles_set {
    use std::collections::HashSet;

    use reth_trie_common::Nibbles;
    use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S>(map: &HashSet<Nibbles>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut storage_nodes =
            Vec::from_iter(map.iter().map(|elem| alloy_primitives::hex::encode(elem.pack())));
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
#[cfg(feature = "serde")]
mod serde_nibbles_map {
    use std::{collections::HashMap, marker::PhantomData};

    use alloy_primitives::hex;
    use reth_trie_common::Nibbles;
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

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map with hex-encoded Nibbles keys")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut result = HashMap::with_capacity(map.size_hint().unwrap_or(0));

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

/// Bincode-compatible trie updates type serde implementations.
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub mod serde_bincode_compat {
    use std::{
        borrow::Cow,
        collections::{HashMap, HashSet},
    };

    use alloy_primitives::B256;
    use reth_trie_common::{BranchNodeCompact, Nibbles};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::TrieUpdates`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_trie::{serde_bincode_compat, updates::TrieUpdates};
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
        storage_tries: HashMap<B256, StorageTrieUpdates<'a>>,
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
    /// use reth_trie::{serde_bincode_compat, updates::StorageTrieUpdates};
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

    #[cfg(test)]
    mod tests {
        use crate::updates::StorageTrieUpdates;

        use super::super::{serde_bincode_compat, TrieUpdates};

        use alloy_primitives::B256;
        use reth_trie_common::{BranchNodeCompact, Nibbles};
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        #[test]
        fn test_trie_updates_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::TrieUpdates")]
                trie_updates: TrieUpdates,
            }

            let mut data = Data { trie_updates: TrieUpdates::default() };
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates.removed_nodes.insert(Nibbles::from_vec(vec![0x0b, 0x0e, 0x0e, 0x0f]));
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates.account_nodes.insert(
                Nibbles::from_vec(vec![0x0d, 0x0e, 0x0a, 0x0d]),
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
                #[serde_as(as = "serde_bincode_compat::StorageTrieUpdates")]
                trie_updates: StorageTrieUpdates,
            }

            let mut data = Data { trie_updates: StorageTrieUpdates::default() };
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates.removed_nodes.insert(Nibbles::from_vec(vec![0x0b, 0x0e, 0x0e, 0x0f]));
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.trie_updates.storage_nodes.insert(
                Nibbles::from_vec(vec![0x0d, 0x0e, 0x0a, 0x0d]),
                BranchNodeCompact::default(),
            );
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }
    }
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;

    #[test]
    fn test_trie_updates_serde_roundtrip() {
        let mut default_updates = TrieUpdates::default();
        let updates_serialized = serde_json::to_string(&default_updates).unwrap();
        let updates_deserialized: TrieUpdates = serde_json::from_str(&updates_serialized).unwrap();
        assert_eq!(updates_deserialized, default_updates);

        default_updates.removed_nodes.insert(Nibbles::from_vec(vec![0x0b, 0x0e, 0x0e, 0x0f]));
        let updates_serialized = serde_json::to_string(&default_updates).unwrap();
        let updates_deserialized: TrieUpdates = serde_json::from_str(&updates_serialized).unwrap();
        assert_eq!(updates_deserialized, default_updates);

        default_updates
            .account_nodes
            .insert(Nibbles::from_vec(vec![0x0d, 0x0e, 0x0a, 0x0d]), BranchNodeCompact::default());
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

        default_updates.removed_nodes.insert(Nibbles::from_vec(vec![0x0b, 0x0e, 0x0e, 0x0f]));
        let updates_serialized = serde_json::to_string(&default_updates).unwrap();
        let updates_deserialized: StorageTrieUpdates =
            serde_json::from_str(&updates_serialized).unwrap();
        assert_eq!(updates_deserialized, default_updates);

        default_updates
            .storage_nodes
            .insert(Nibbles::from_vec(vec![0x0d, 0x0e, 0x0a, 0x0d]), BranchNodeCompact::default());
        let updates_serialized = serde_json::to_string(&default_updates).unwrap();
        let updates_deserialized: StorageTrieUpdates =
            serde_json::from_str(&updates_serialized).unwrap();
        assert_eq!(updates_deserialized, default_updates);
    }
}
