/// The implementation of forward-only in memory cursor over the entries.
///
/// The cursor operates under the assumption that the supplied collection is pre-sorted.
#[derive(Debug)]
pub struct ForwardInMemoryCursor<'a, K, V> {
    /// The reference to the pre-sorted collection of entries.
    entries: std::slice::Iter<'a, (K, V)>,
    is_empty: bool,
}

impl<'a, K, V> ForwardInMemoryCursor<'a, K, V> {
    /// Create new forward cursor positioned at the beginning of the collection.
    ///
    /// The cursor expects all of the entries to have been sorted in advance.
    #[inline]
    pub fn new(entries: &'a [(K, V)]) -> Self {
        Self { entries: entries.iter(), is_empty: entries.is_empty() }
    }

    /// Returns `true` if the cursor is empty, regardless of its position.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.is_empty
    }

    /// Returns the last entry iterated to. Returns `None` if never iterated or if exhausted.
    #[inline]
    pub fn current(&self) -> Option<&'a (K, V)> {
        self.entries.clone().next()
    }
}

impl<'a, K, V> Iterator for ForwardInMemoryCursor<'a, K, V> {
    type Item = &'a (K, V);

    fn next(&mut self) -> Option<&'a (K, V)> {
        self.entries.next();
        self.current()
    }
}

impl<'a, K, V> ForwardInMemoryCursor<'a, K, V>
where
    K: PartialOrd + Clone,
    V: Clone,
{
    /// Returns the first entry from the current cursor position that's greater or equal to the
    /// provided key. This method advances the cursor forward.
    pub fn seek(&mut self, key: &K) -> Option<&'a (K, V)> {
        self.advance_while(|k| k < key)
    }

    /// Returns the first entry from the current cursor position that's greater than the provided
    /// key. This method advances the cursor forward.
    pub fn first_after(&mut self, key: &K) -> Option<&'a (K, V)> {
        self.advance_while(|k| k <= key)
    }

    /// Advances the cursor forward while `predicate` returns `true` or until the collection is
    /// exhausted.
    ///
    /// Returns the first entry for which `predicate` returns `false` or `None`. The cursor will
    /// point to the returned entry.
    fn advance_while(&mut self, predicate: impl Fn(&'a K) -> bool) -> Option<&'a (K, V)> {
        loop {
            if self.current().is_some_and(|(k, _)| predicate(k)) {
                self.entries.next();
            } else {
                break;
            }
        }
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor() {
        let mut cursor = ForwardInMemoryCursor::new(&[(1, ()), (2, ()), (3, ()), (4, ()), (5, ())]);

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
}
