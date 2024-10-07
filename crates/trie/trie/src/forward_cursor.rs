/// The implementation of forward-only in memory cursor over the entries.
/// The cursor operates under the assumption that the supplied collection is pre-sorted.
#[derive(Debug)]
pub struct ForwardInMemoryCursor<'a, K, V> {
    /// The reference to the pre-sorted collection of entries.
    entries: &'a Vec<(K, V)>,
    /// The index where cursor is currently positioned.
    index: usize,
}

impl<'a, K, V> ForwardInMemoryCursor<'a, K, V> {
    /// Create new forward cursor positioned at the beginning of the collection.
    /// The cursor expects all of the entries have been sorted in advance.
    pub const fn new(entries: &'a Vec<(K, V)>) -> Self {
        Self { entries, index: 0 }
    }

    /// Returns `true` if the cursor is empty, regardless of its position.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<K, V> ForwardInMemoryCursor<'_, K, V>
where
    K: PartialOrd + Clone,
    V: Clone,
{
    /// Advances the cursor forward while `comparator` returns `true` or until the collection is
    /// exhausted. Returns the first entry for which `comparator` returns `false` or `None`.
    fn advance_while_false(&mut self, comparator: impl Fn(&K) -> bool) -> Option<(K, V)> {
        let mut entry = self.entries.get(self.index);
        while entry.map_or(false, |entry| comparator(&entry.0)) {
            self.index += 1;
            entry = self.entries.get(self.index);
        }
        entry.cloned()
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
