/// The implementation of forward-only in memory cursor over the entries.
/// The cursor operates under the assumption that the supplied collection is pre-sorted.
#[derive(Debug)]
pub struct ForwardInMemoryCursor<'a, K, V> {
    /// The reference to the pre-sorted collection of entries.
    entries: &'a [(K, V)],
    /// The index where cursor is currently positioned.
    index: usize,
}

impl<'a, K, V> ForwardInMemoryCursor<'a, K, V> {
    /// Create new forward cursor positioned at the beginning of the collection.
    /// The cursor expects all of the entries have been sorted in advance.
    pub const fn new(entries: &'a [(K, V)]) -> Self {
        Self { entries, index: 0 }
    }

    /// Returns `true` if the cursor is empty, regardless of its position.
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<K, V> ForwardInMemoryCursor<'_, K, V>
where
    K: Ord + Clone + PartialOrd,
    V: Clone + PartialOrd,
{
    /// Advances the cursor forward while `comparator` returns `true` or until the collection is
    /// exhausted. Returns the first entry for which `comparator` returns `false` or `None`.
    fn advance_while_false(&mut self, comparator: impl Fn(&K) -> bool) -> Option<(K, V)> {
        debug_assert!(self.entries.is_sorted());

        if self.index >= self.entries.len() {
            return None;
        }

        // use binary search to get to a false element
        let pos = match self.entries[self.index..].binary_search_by(|(k, _)| {
            if comparator(k) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        }) {
            Ok(found_pos) => self.index + found_pos,
            Err(insert_pos) => self.index + insert_pos,
        };

        // walk backwards to find the first false element
        let mut first_false = pos;
        while first_false > self.index && !comparator(&self.entries[first_false - 1].0) {
            first_false -= 1;
        }

        self.index = first_false;
        self.entries.get(self.index).cloned()
    }

    /// Returns the first entry from the current cursor position that's greater or equal to the
    /// provided key. This method advances the cursor forward.
    pub fn seek(&mut self, key: &K) -> Option<(K, V)> {
        self.advance_while_false(|k| k < key)
    }

    /// Returns the first entry from the current cursor position that's greater than the provided
    /// key. This method advances the cursor forward.
    pub fn first_after(&mut self, key: &K) -> Option<(K, V)> {
        self.advance_while_false(|k| k <= key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_advance_while_false() {
        let cases = vec![
            ("Basic unique keys", vec![(1, "a"), (2, "b"), (3, "c"), (4, "d")], 2, Some((2, "b"))),
            (
                "Duplicate keys",
                vec![(1, "a"), (2, "b1"), (2, "b2"), (2, "b3"), (3, "c")],
                2,
                Some((2, "b1")),
            ),
            ("Gap values", vec![(1, "a"), (2, "b"), (4, "d"), (5, "e")], 3, Some((4, "d"))),
            ("Empty cursor", vec![], 1, None),
            ("Single element", vec![(1, "a")], 1, Some((1, "a"))),
            ("Already positioned", vec![(1, "a"), (2, "b"), (3, "c")], 1, Some((1, "a"))),
        ];

        for (name, entries, search_key, expected) in cases {
            let mut cursor = ForwardInMemoryCursor::new(&entries);
            let result = cursor.advance_while_false(|k| k < &search_key);
            assert_eq!(result, expected, "Failed case: {}", name);
        }
    }

    #[test]
    fn test_forward_only_movement() {
        let entries = vec![(1, "a"), (2, "b"), (3, "c"), (4, "d")];
        let mut cursor = ForwardInMemoryCursor::new(&entries);

        assert_eq!(cursor.seek(&3), Some((3, "c")));

        // Trying to seek to lower values should not move backwards
        assert_eq!(cursor.seek(&1), Some((3, "c")));
        assert_eq!(cursor.seek(&2), Some((3, "c")));

        // test if forward seeking still works
        assert_eq!(cursor.seek(&4), Some((4, "d")));
    }

    #[test]
    fn test_first_after() {
        let entries = vec![(1, "a"), (2, "b"), (2, "b2"), (3, "c")];
        let mut cursor = ForwardInMemoryCursor::new(&entries);

        // test finding first after with duplicates
        assert_eq!(cursor.first_after(&2), Some((3, "c")));

        // test no value after
        assert_eq!(cursor.first_after(&3), None);
    }
}
