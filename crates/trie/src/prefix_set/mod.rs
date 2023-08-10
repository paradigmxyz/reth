use reth_primitives::trie::Nibbles;
use std::rc::Rc;

mod loader;
pub use loader::{LoadedPrefixSets, PrefixSetLoader};

/// A container for efficiently storing and checking for the presence of key prefixes.
///
/// This data structure stores a set of `Nibbles` and provides methods to insert
/// new elements and check whether any existing element has a given prefix.
///
/// Internally, this implementation uses a `Vec` and aims to act like a `BTreeSet` in being both
/// sorted and deduplicated. It does this by keeping a `sorted` flag. The `sorted` flag represents
/// whether or not the `Vec` is definitely sorted. When a new element is added, it is set to
/// `false.`. The `Vec` is sorted and deduplicated when `sorted` is `false` and:
///  * An element is being checked for inclusion (`contains`), or
///  * The set is being converted into an immutable `PrefixSet` (`freeze`)
///
/// This means that a `PrefixSet` will always be sorted and deduplicated when constructed from a
/// `PrefixSetMut`.
///
/// # Examples
///
/// ```
/// use reth_trie::prefix_set::PrefixSetMut;
///
/// let mut prefix_set = PrefixSetMut::default();
/// prefix_set.insert(b"key1");
/// prefix_set.insert(b"key2");
///
/// assert_eq!(prefix_set.contains(b"key"), true);
/// ```
#[derive(Debug, Default, Clone)]
pub struct PrefixSetMut {
    keys: Vec<Nibbles>,
    sorted: bool,
    index: usize,
}

impl PrefixSetMut {
    /// Returns `true` if any of the keys in the set has the given prefix or
    /// if the given prefix is a prefix of any key in the set.
    pub fn contains<T: Into<Nibbles>>(&mut self, prefix: T) -> bool {
        if !self.sorted {
            self.keys.sort();
            self.keys.dedup();
            self.sorted = true;
        }

        let prefix = prefix.into();

        while self.index > 0 && self.keys[self.index] > prefix {
            self.index -= 1;
        }

        for (idx, key) in self.keys[self.index..].iter().enumerate() {
            if key.has_prefix(&prefix) {
                self.index += idx;
                return true
            }

            if key > &prefix {
                self.index += idx;
                return false
            }
        }

        false
    }

    /// Inserts the given `nibbles` into the set.
    pub fn insert<T: Into<Nibbles>>(&mut self, nibbles: T) {
        self.sorted = false;
        self.keys.push(nibbles.into());
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

        PrefixSet { keys: Rc::new(self.keys), index: self.index }
    }
}

/// A sorted prefix set that has an immutable _sorted_ list of unique keys.
///
/// See also [PrefixSetMut::freeze].
#[derive(Debug, Default, Clone)]
pub struct PrefixSet {
    keys: Rc<Vec<Nibbles>>,
    index: usize,
}

impl PrefixSet {
    /// Returns `true` if any of the keys in the set has the given prefix or
    /// if the given prefix is a prefix of any key in the set.
    #[inline]
    pub fn contains<T: Into<Nibbles>>(&mut self, prefix: T) -> bool {
        let prefix = prefix.into();

        while self.index > 0 && self.keys[self.index] > prefix {
            self.index -= 1;
        }

        for (idx, key) in self.keys[self.index..].iter().enumerate() {
            if key.has_prefix(&prefix) {
                self.index += idx;
                return true
            }

            if key > &prefix {
                self.index += idx;
                return false
            }
        }

        false
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains_with_multiple_inserts_and_duplicates() {
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(b"123");
        prefix_set.insert(b"124");
        prefix_set.insert(b"456");
        prefix_set.insert(b"123"); // Duplicate

        assert!(prefix_set.contains(b"12"));
        assert!(prefix_set.contains(b"45"));
        assert!(!prefix_set.contains(b"78"));
        assert_eq!(prefix_set.len(), 3); // Length should be 3 (excluding duplicate)
    }
}
