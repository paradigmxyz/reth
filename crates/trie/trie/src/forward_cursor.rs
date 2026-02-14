/// The implementation of forward-only in memory cursor over the entries.
///
/// The cursor operates under the assumption that the supplied collection is pre-sorted.
#[derive(Debug)]
pub struct ForwardInMemoryCursor<'a, K, V> {
    /// The reference to the pre-sorted collection of entries.
    entries: &'a [(K, V)],
    /// Current index in the collection.
    idx: usize,
}

impl<'a, K, V> ForwardInMemoryCursor<'a, K, V> {
    /// Create new forward cursor positioned at the beginning of the collection.
    ///
    /// The cursor expects all of the entries to have been sorted in advance.
    #[inline]
    pub const fn new(entries: &'a [(K, V)]) -> Self {
        Self { entries, idx: 0 }
    }

    /// Returns `true` if the cursor is empty, regardless of its position.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns `true` if any entry satisfies the predicate.
    #[inline]
    pub fn has_any<F>(&self, predicate: F) -> bool
    where
        F: Fn(&(K, V)) -> bool,
    {
        self.entries.iter().any(predicate)
    }

    /// Returns the current entry pointed to be the cursor, or `None` if no entries are left.
    #[inline]
    pub fn current(&self) -> Option<&(K, V)> {
        self.entries.get(self.idx)
    }

    /// Resets the cursor to the beginning of the collection.
    #[inline]
    pub const fn reset(&mut self) {
        self.idx = 0;
    }

    #[inline]
    fn next(&mut self) -> Option<&(K, V)> {
        let entry = self.entries.get(self.idx)?;
        self.idx += 1;
        Some(entry)
    }
}

/// Threshold for remaining entries above which binary search is used instead of linear scan.
/// For small slices, linear scan has better cache locality and lower overhead.
const BINARY_SEARCH_THRESHOLD: usize = 64;

impl<K: Ord, V> ForwardInMemoryCursor<'_, K, V> {
    /// Returns the first entry from the current cursor position that's greater or equal to the
    /// provided key. This method advances the cursor forward.
    pub fn seek(&mut self, key: &K) -> Option<&(K, V)> {
        self.advance_while(|k| k < key)
    }

    /// Returns the first entry from the current cursor position that's greater than the provided
    /// key. This method advances the cursor forward.
    pub fn first_after(&mut self, key: &K) -> Option<&(K, V)> {
        self.advance_while(|k| k <= key)
    }

    /// Advances the cursor forward while `predicate` returns `true` or until the collection is
    /// exhausted.
    ///
    /// Uses binary search for large remaining slices (>= 64 entries), linear scan for small ones.
    ///
    /// Returns the first entry for which `predicate` returns `false` or `None`. The cursor will
    /// point to the returned entry.
    fn advance_while(&mut self, predicate: impl Fn(&K) -> bool) -> Option<&(K, V)> {
        let remaining = self.entries.len().saturating_sub(self.idx);
        if remaining >= BINARY_SEARCH_THRESHOLD {
            let slice = &self.entries[self.idx..];
            let pos = slice.partition_point(|(k, _)| predicate(k));
            self.idx += pos;
        } else {
            while self.current().is_some_and(|(k, _)| predicate(k)) {
                self.next();
            }
        }
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_small() {
        let mut cursor = ForwardInMemoryCursor::new(&[(1, ()), (2, ()), (3, ()), (4, ()), (5, ())]);
        assert_eq!(cursor.current(), Some(&(1, ())));

        assert_eq!(cursor.seek(&0), Some(&(1, ())));
        assert_eq!(cursor.current(), Some(&(1, ())));

        assert_eq!(cursor.seek(&3), Some(&(3, ())));
        assert_eq!(cursor.current(), Some(&(3, ())));

        assert_eq!(cursor.seek(&3), Some(&(3, ())));
        assert_eq!(cursor.current(), Some(&(3, ())));

        assert_eq!(cursor.seek(&4), Some(&(4, ())));
        assert_eq!(cursor.current(), Some(&(4, ())));

        assert_eq!(cursor.seek(&6), None);
        assert_eq!(cursor.current(), None);
    }

    #[test]
    fn test_cursor_large_binary_search() {
        // Create a large enough collection to trigger binary search
        let entries: Vec<(i32, ())> = (0..200).map(|i| (i * 2, ())).collect();
        let mut cursor = ForwardInMemoryCursor::new(&entries);

        // Seek to beginning
        assert_eq!(cursor.seek(&0), Some(&(0, ())));
        assert_eq!(cursor.idx, 0);

        // Seek to middle (should use binary search)
        assert_eq!(cursor.seek(&100), Some(&(100, ())));
        assert_eq!(cursor.idx, 50);

        // Seek to non-existent key (should find next greater)
        assert_eq!(cursor.seek(&101), Some(&(102, ())));
        assert_eq!(cursor.idx, 51);

        // Seek to end
        assert_eq!(cursor.seek(&398), Some(&(398, ())));
        assert_eq!(cursor.idx, 199);

        // Seek past end
        assert_eq!(cursor.seek(&1000), None);
    }

    #[test]
    fn test_first_after_large() {
        let entries: Vec<(i32, ())> = (0..200).map(|i| (i * 2, ())).collect();
        let mut cursor = ForwardInMemoryCursor::new(&entries);

        // first_after should find strictly greater
        assert_eq!(cursor.first_after(&0), Some(&(2, ())));
        assert_eq!(cursor.idx, 1);

        // Reset and test from beginning
        cursor.reset();
        assert_eq!(cursor.first_after(&99), Some(&(100, ())));

        // first_after on exact match
        cursor.reset();
        assert_eq!(cursor.first_after(&100), Some(&(102, ())));
    }

    #[test]
    fn test_cursor_consistency() {
        // Verify binary search and linear scan produce same results
        let entries: Vec<(i32, ())> = (0..200).map(|i| (i * 3, ())).collect();

        for search_key in [0, 1, 3, 50, 150, 299, 300, 597, 598, 599, 1000] {
            // Test with fresh cursor (binary search path)
            let mut cursor1 = ForwardInMemoryCursor::new(&entries);
            let result1 = cursor1.seek(&search_key);

            // Manually advance to trigger linear path by getting close first
            let mut cursor2 = ForwardInMemoryCursor::new(&entries);
            if search_key > 100 {
                cursor2.seek(&(search_key - 50));
            }
            let result2 = cursor2.seek(&search_key);

            assert_eq!(
                result1, result2,
                "Mismatch for key {search_key}: binary={result1:?}, linear={result2:?}"
            );
        }
    }
}
