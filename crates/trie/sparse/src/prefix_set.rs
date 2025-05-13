use alloc::{sync::Arc, vec::Vec};
use alloy_primitives::map::{B256Map, B256Set};

use crate::PackedNibbles;

/// Collection of mutable prefix sets.
#[derive(Clone, Default, Debug)]
pub struct TriePrefixSetsMut {
    /// A set of account prefixes that have changed.
    pub account_prefix_set: PrefixSetMut,
    /// A map containing storage changes with the hashed address as key and a set of storage key
    /// prefixes as the value.
    pub storage_prefix_sets: B256Map<PrefixSetMut>,
    /// A set of hashed addresses of destroyed accounts.
    pub destroyed_accounts: B256Set,
}

impl TriePrefixSetsMut {
    /// Extends prefix sets with contents of another prefix set.
    pub fn extend(&mut self, other: Self) {
        self.account_prefix_set.extend(other.account_prefix_set);
        for (hashed_address, prefix_set) in other.storage_prefix_sets {
            self.storage_prefix_sets.entry(hashed_address).or_default().extend(prefix_set);
        }
        self.destroyed_accounts.extend(other.destroyed_accounts);
    }

    /// Returns a `TriePrefixSets` with the same elements as these sets.
    ///
    /// If not yet sorted, the elements will be sorted and deduplicated.
    pub fn freeze(self) -> TriePrefixSets {
        TriePrefixSets {
            account_prefix_set: self.account_prefix_set.freeze(),
            storage_prefix_sets: self
                .storage_prefix_sets
                .into_iter()
                .map(|(hashed_address, prefix_set)| (hashed_address, prefix_set.freeze()))
                .collect(),
            destroyed_accounts: self.destroyed_accounts,
        }
    }
}

/// Collection of trie prefix sets.
#[derive(Default, Debug)]
pub struct TriePrefixSets {
    /// A set of account prefixes that have changed.
    pub account_prefix_set: PrefixSet,
    /// A map containing storage changes with the hashed address as key and a set of storage key
    /// prefixes as the value.
    pub storage_prefix_sets: B256Map<PrefixSet>,
    /// A set of hashed addresses of destroyed accounts.
    pub destroyed_accounts: B256Set,
}

#[derive(PartialEq, Eq, Clone, Default, Debug)]
pub struct PrefixSetMut {
    /// Flag indicating that any entry should be considered changed.
    /// If set, the keys will be discarded.
    all: bool,
    keys: Vec<PackedNibbles>,
}

impl<I> From<I> for PrefixSetMut
where
    I: IntoIterator<Item = PackedNibbles>,
{
    fn from(value: I) -> Self {
        Self { all: false, keys: value.into_iter().collect() }
    }
}

impl PrefixSetMut {
    /// Create [`PrefixSetMut`] with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { all: false, keys: Vec::with_capacity(capacity) }
    }

    /// Create [`PrefixSetMut`] that considers all key changed.
    pub const fn all() -> Self {
        Self { all: true, keys: Vec::new() }
    }

    /// Inserts the given `nibbles` into the set.
    pub fn insert(&mut self, nibbles: PackedNibbles) {
        self.keys.push(nibbles);
    }

    /// Extend prefix set with contents of another prefix set.
    pub fn extend(&mut self, other: Self) {
        self.all |= other.all;
        self.keys.extend(other.keys);
    }

    /// Extend prefix set keys with contents of provided iterator.
    pub fn extend_keys<I>(&mut self, keys: I)
    where
        I: IntoIterator<Item = PackedNibbles>,
    {
        self.keys.extend(keys);
    }

    /// Returns the number of elements in the set.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Returns `true` if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Returns a `PrefixSet` with the same elements as this set.
    ///
    /// If not yet sorted, the elements will be sorted and deduplicated.
    pub fn freeze(mut self) -> PrefixSet {
        if self.all {
            PrefixSet { index: 0, all: true, keys: Arc::new(Vec::new()) }
        } else {
            self.keys.sort_unstable();
            self.keys.dedup();
            // We need to shrink in both the sorted and non-sorted cases because deduping may have
            // occurred either on `freeze`, or during `contains`.
            self.keys.shrink_to_fit();
            PrefixSet { index: 0, all: false, keys: Arc::new(self.keys) }
        }
    }
}

/// A sorted prefix set that has an immutable _sorted_ list of unique keys.
///
/// See also [`PrefixSetMut::freeze`].
#[derive(Debug, Default, Clone)]
pub struct PrefixSet {
    /// Flag indicating that any entry should be considered changed.
    all: bool,
    index: usize,
    keys: Arc<Vec<PackedNibbles>>,
}

impl PrefixSet {
    /// Returns `true` if any of the keys in the set has the given prefix
    #[inline]
    pub fn contains(&mut self, prefix: &PackedNibbles) -> bool {
        if self.all {
            return true
        }

        while self.index > 0 && &self.keys[self.index] > prefix {
            self.index -= 1;
        }

        for (idx, key) in self.keys[self.index..].iter().enumerate() {
            if key.starts_with(prefix) {
                self.index += idx;
                return true
            }

            if key > prefix {
                self.index += idx;
                return false
            }
        }

        false
    }

    /// Returns an iterator over reference to _all_ nibbles regardless of cursor position.
    pub fn iter(&self) -> core::slice::Iter<'_, PackedNibbles> {
        self.keys.iter()
    }

    /// Returns the number of elements in the set.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Returns `true` if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

impl<'a> IntoIterator for &'a PrefixSet {
    type Item = &'a PackedNibbles;
    type IntoIter = core::slice::Iter<'a, PackedNibbles>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
