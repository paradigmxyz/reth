/// The implementation of forward-only in memory cursor over the entries.
#[derive(Debug)]
pub struct ForwardCursor<'a, K, V> {
    /// The reference to the collection of entries
    entries: &'a Vec<(K, V)>,
    /// The index where cursor is currently positioned.
    index: usize,
}

impl<'a, K, V> ForwardCursor<'a, K, V> {
    /// Create new forward cursor positioned at the beginning of the collection.
    pub const fn new(entries: &'a Vec<(K, V)>) -> Self {
        Self { entries, index: 0 }
    }
}

impl<'a, K, V> ForwardCursor<'a, K, V>
where
    K: PartialOrd,
{
    /// Returns the first entry from the current cursor position that's greater or equal to the
    /// provided key. This method advance the cursor forward.
    pub fn seek(&mut self, key: &K) -> Option<&(K, V)> {
        let mut entry = self.entries.get(self.index);
        while entry.map_or(false, |(k, _)| k < key) {
            self.index += 1;
            entry = self.entries.get(self.index);
        }
        entry
    }

    /// Returns the first entry from the current cursor position that's greater than the provided
    /// key. This method advance the cursor forward.
    pub fn first_after(&mut self, key: &K) -> Option<&(K, V)> {
        let mut entry = self.entries.get(self.index);
        while entry.map_or(false, |(k, _)| k <= key) {
            self.index += 1;
            entry = self.entries.get(self.index);
        }
        entry
    }
}
