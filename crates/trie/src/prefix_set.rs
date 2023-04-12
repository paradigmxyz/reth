use crate::Nibbles;
use std::collections::BTreeSet;

/// A container for efficiently storing and checking for the presence of key prefixes.
///
/// This data structure stores a set of `Nibbles` and provides methods to insert
/// new elements and check whether any existing element has a given prefix.
///
/// Internally, this implementation uses a `BTreeSet` to store the `Nibbles`, which
/// ensures that they are always sorted and deduplicated.
///
/// # Examples
///
/// ```
/// use reth_trie::prefix_set::PrefixSet;
///
/// let mut prefix_set = PrefixSet::default();
/// prefix_set.insert(b"key1");
/// prefix_set.insert(b"key2");
///
/// assert_eq!(prefix_set.contains(b"key"), true);
/// ```
#[derive(Debug, Default, Clone)]
pub struct PrefixSet {
    keys: BTreeSet<Nibbles>,
}

impl PrefixSet {
    /// Returns `true` if any of the keys in the set has the given prefix or
    /// if the given prefix is a prefix of any key in the set.
    pub fn contains<T: Into<Nibbles>>(&self, prefix: T) -> bool {
        let prefix = prefix.into();
        self.keys.iter().any(|key| key.has_prefix(&prefix))
    }

    /// Inserts the given `nibbles` into the set.
    pub fn insert<T: Into<Nibbles>>(&mut self, nibbles: T) {
        self.keys.insert(nibbles.into());
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
        let mut prefix_set = PrefixSet::default();
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
