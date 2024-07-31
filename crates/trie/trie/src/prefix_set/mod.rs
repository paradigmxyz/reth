use crate::Nibbles;
use reth_primitives::B256;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

mod loader;
pub use loader::PrefixSetLoader;

/// Collection of mutable prefix sets.
#[derive(Clone, Default, Debug)]
pub struct TriePrefixSetsMut {
    /// A set of account prefixes that have changed.
    pub account_prefix_set: PrefixSetMut,
    /// A map containing storage changes with the hashed address as key and a set of storage key
    /// prefixes as the value.
    pub storage_prefix_sets: HashMap<B256, PrefixSetMut>,
    /// A set of hashed addresses of destroyed accounts.
    pub destroyed_accounts: HashSet<B256>,
}

impl TriePrefixSetsMut {
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
    pub storage_prefix_sets: HashMap<B256, PrefixSet>,
    /// A set of hashed addresses of destroyed accounts.
    pub destroyed_accounts: HashSet<B256>,
}

/// A container for efficiently storing and checking for the presence of key prefixes.
///
/// This data structure stores a set of `Nibbles` and provides methods to insert
/// new elements and check whether any existing element has a given prefix.
///
/// Internally, this implementation uses a `Vec` and aims to act like a `BTreeSet` in being both
/// sorted and deduplicated. It does this by keeping a `sorted` flag. The `sorted` flag represents
/// whether or not the `Vec` is definitely sorted. When a new element is added, it is set to
/// `false.`. The `Vec` is sorted and deduplicated when `sorted` is `true` and:
///  * An element is being checked for inclusion (`contains`), or
///  * The set is being converted into an immutable `PrefixSet` (`freeze`)
///
/// This means that a `PrefixSet` will always be sorted and deduplicated when constructed from a
/// `PrefixSetMut`.
///
/// # Examples
///
/// ```
/// use reth_trie::{prefix_set::PrefixSetMut, Nibbles};
///
/// let mut prefix_set = PrefixSetMut::default();
/// prefix_set.insert(Nibbles::from_nibbles_unchecked(&[0xa, 0xb]));
/// prefix_set.insert(Nibbles::from_nibbles_unchecked(&[0xa, 0xb, 0xc]));
/// assert!(prefix_set.contains(&[0xa, 0xb]));
/// assert!(prefix_set.contains(&[0xa, 0xb, 0xc]));
/// ```
#[derive(Clone, Default, Debug)]
pub struct PrefixSetMut {
    keys: Vec<Nibbles>,
    sorted: bool,
    index: usize,
}

impl<I> From<I> for PrefixSetMut
where
    I: IntoIterator<Item = Nibbles>,
{
    fn from(value: I) -> Self {
        Self { keys: value.into_iter().collect(), ..Default::default() }
    }
}

impl PrefixSetMut {
    /// Create [`PrefixSetMut`] with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { keys: Vec::with_capacity(capacity), ..Default::default() }
    }

    /// Returns `true` if any of the keys in the set has the given prefix or
    /// if the given prefix is a prefix of any key in the set.
    pub fn contains(&mut self, prefix: &[u8]) -> bool {
        if !self.sorted {
            self.keys.sort();
            self.keys.dedup();
            self.sorted = true;
        }

        while self.index > 0 && self.keys[self.index] > *prefix {
            self.index -= 1;
        }

        for (idx, key) in self.keys[self.index..].iter().enumerate() {
            if key.has_prefix(prefix) {
                self.index += idx;
                return true
            }

            if *key > *prefix {
                self.index += idx;
                return false
            }
        }

        false
    }

    /// Inserts the given `nibbles` into the set.
    pub fn insert(&mut self, nibbles: Nibbles) {
        self.sorted = false;
        self.keys.push(nibbles);
    }

    /// Extend prefix set keys with contents of provided iterator.
    pub fn extend<I>(&mut self, nibbles_iter: I)
    where
        I: IntoIterator<Item = Nibbles>,
    {
        self.sorted = false;
        self.keys.extend(nibbles_iter);
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
        if !self.sorted {
            self.keys.sort();
            self.keys.dedup();
        }

        // we need to shrink in both the sorted and non-sorted cases because deduping may have
        // occurred either on `freeze`, or during `contains`.
        self.keys.shrink_to_fit();
        PrefixSet { keys: Arc::new(self.keys), index: self.index }
    }
}

/// A sorted prefix set that has an immutable _sorted_ list of unique keys.
///
/// See also [`PrefixSetMut::freeze`].
#[derive(Debug, Default, Clone)]
pub struct PrefixSet {
    keys: Arc<Vec<Nibbles>>,
    index: usize,
}

impl PrefixSet {
    /// Returns `true` if any of the keys in the set has the given prefix or
    /// if the given prefix is a prefix of any key in the set.
    #[inline]
    pub fn contains(&mut self, prefix: &Nibbles) -> bool {
        while self.index > 0 && &self.keys[self.index] > prefix {
            self.index -= 1;
        }

        for (idx, key) in self.keys[self.index..].iter().enumerate() {
            if key.has_prefix(prefix) {
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
    pub fn iter(&self) -> core::slice::Iter<'_, Nibbles> {
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
    type IntoIter = std::slice::Iter<'a, reth_trie_common::Nibbles>;
    type Item = &'a reth_trie_common::Nibbles;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains_with_multiple_inserts_and_duplicates() {
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::from_nibbles([1, 2, 3]));
        prefix_set.insert(Nibbles::from_nibbles([1, 2, 4]));
        prefix_set.insert(Nibbles::from_nibbles([4, 5, 6]));
        prefix_set.insert(Nibbles::from_nibbles([1, 2, 3])); // Duplicate

        assert!(prefix_set.contains(&[1, 2]));
        assert!(prefix_set.contains(&[4, 5]));
        assert!(!prefix_set.contains(&[7, 8]));
        assert_eq!(prefix_set.len(), 3); // Length should be 3 (excluding duplicate)
    }

    #[test]
    fn test_freeze_shrinks_capacity() {
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::from_nibbles([1, 2, 3]));
        prefix_set.insert(Nibbles::from_nibbles([1, 2, 4]));
        prefix_set.insert(Nibbles::from_nibbles([4, 5, 6]));
        prefix_set.insert(Nibbles::from_nibbles([1, 2, 3])); // Duplicate

        assert!(prefix_set.contains(&[1, 2]));
        assert!(prefix_set.contains(&[4, 5]));
        assert!(!prefix_set.contains(&[7, 8]));
        assert_eq!(prefix_set.keys.len(), 3); // Length should be 3 (excluding duplicate)
        assert_eq!(prefix_set.keys.capacity(), 4); // Capacity should be 4 (including duplicate)

        let frozen = prefix_set.freeze();
        assert_eq!(frozen.keys.len(), 3); // Length should be 3 (excluding duplicate)
        assert_eq!(frozen.keys.capacity(), 3); // Capacity should be 3 after shrinking
    }

    #[test]
    fn test_freeze_shrinks_existing_capacity() {
        // do the above test but with preallocated capacity
        let mut prefix_set = PrefixSetMut::with_capacity(101);
        prefix_set.insert(Nibbles::from_nibbles([1, 2, 3]));
        prefix_set.insert(Nibbles::from_nibbles([1, 2, 4]));
        prefix_set.insert(Nibbles::from_nibbles([4, 5, 6]));
        prefix_set.insert(Nibbles::from_nibbles([1, 2, 3])); // Duplicate

        assert!(prefix_set.contains(&[1, 2]));
        assert!(prefix_set.contains(&[4, 5]));
        assert!(!prefix_set.contains(&[7, 8]));
        assert_eq!(prefix_set.keys.len(), 3); // Length should be 3 (excluding duplicate)
        assert_eq!(prefix_set.keys.capacity(), 101); // Capacity should be 101 (including duplicate)

        let frozen = prefix_set.freeze();
        assert_eq!(frozen.keys.len(), 3); // Length should be 3 (excluding duplicate)
        assert_eq!(frozen.keys.capacity(), 3); // Capacity should be 3 after shrinking
    }
}
