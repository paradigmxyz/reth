use alloc::{boxed::Box, vec::Vec};
use core::cmp::Ordering;
use itertools::Itertools;

type BoxedIterator<'a, T> = Box<dyn Iterator<Item = T> + Send + 'a>;

/// Merge sorted iterators. First occurrence wins for duplicate keys.
///
/// Callers pass iterators in priority order (index 0 = highest priority), so the first iterator's
/// value for a key takes precedence over later iterators.
pub(crate) fn kway_merge_sorted<'a, K, V, I>(iterators: I) -> BoxedIterator<'a, (K, V)>
where
    K: Ord + Send + 'a,
    V: Send + 'a,
    I: IntoIterator<Item = BoxedIterator<'a, (K, V)>>,
    I::IntoIter: Send + 'a,
{
    let mut iterators = iterators.into_iter();
    let Some(first) = iterators.next() else { return Box::new(core::iter::empty()) };
    let Some(second) = iterators.next() else { return first };

    Box::new(
        core::iter::once(first)
            .chain(core::iter::once(second))
            .chain(iterators)
            .enumerate()
            .map(|(priority, entries)| entries.map(move |(key, value)| (priority, key, value)))
            .kmerge_by(|(i1, k1, _), (i2, k2, _)| (k1, i1) < (k2, i2))
            .dedup_by(|(_, k1, _), (_, k2, _)| k1 == k2)
            .map(|(_, key, value)| (key, value)),
    )
}

/// Merge sorted iterators and group values with the same key in priority order.
pub(crate) fn kway_merge_sorted_groups<'a, K, V, I>(iterators: I) -> BoxedIterator<'a, (K, Vec<V>)>
where
    K: Ord + Send + 'a,
    V: Send + 'a,
    I: IntoIterator<Item = BoxedIterator<'a, (K, V)>>,
    I::IntoIter: Send + 'a,
{
    let merged = iterators
        .into_iter()
        .enumerate()
        .map(|(priority, entries)| entries.map(move |(key, value)| (priority, key, value)))
        .kmerge_by(|(i1, k1, _), (i2, k2, _)| (k1, i1) < (k2, i2));
    let mut merged = merged.peekable();

    Box::new(core::iter::from_fn(move || {
        let (_, key, value) = merged.next()?;
        let mut values = Vec::new();
        values.push(value);
        while merged.peek().is_some_and(|(_, next_key, _)| next_key == &key) {
            values.push(merged.next().expect("peeked value exists").2);
        }
        Some((key, values))
    }))
}

/// Extend a sorted vector with another sorted vector using 2 pointer merge.
/// Values from `other` take precedence for duplicate keys.
pub(crate) fn extend_sorted_vec<K, V>(target: &mut Vec<(K, V)>, other: &[(K, V)])
where
    K: Clone + Ord,
    V: Clone,
{
    if other.is_empty() {
        return;
    }

    if target.is_empty() {
        target.extend_from_slice(other);
        return;
    }

    // Fast path: non-overlapping ranges - just append
    if target.last().map(|(k, _)| k) < other.first().map(|(k, _)| k) {
        target.extend_from_slice(other);
        return;
    }

    // Move ownership of target to avoid cloning owned elements
    let left = core::mem::take(target);
    let mut out = Vec::with_capacity(left.len() + other.len());

    let mut a = left.into_iter().peekable();
    let mut b = other.iter().peekable();

    while let (Some(aa), Some(bb)) = (a.peek(), b.peek()) {
        match aa.0.cmp(&bb.0) {
            Ordering::Less => {
                out.push(a.next().unwrap());
            }
            Ordering::Greater => {
                out.push(b.next().unwrap().clone());
            }
            Ordering::Equal => {
                // `other` takes precedence for duplicate keys - reuse key from `a`
                let (k, _) = a.next().unwrap();
                out.push((k, b.next().unwrap().1.clone()));
            }
        }
    }

    // Drain remaining: `a` moves, `b` clones
    out.extend(a);
    out.extend(b.cloned());

    *target = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct CloneCounter {
        value: u8,
        count: Arc<AtomicUsize>,
    }

    impl Clone for CloneCounter {
        fn clone(&self) -> Self {
            self.count.fetch_add(1, Ordering::Relaxed);
            Self { value: self.value, count: Arc::clone(&self.count) }
        }
    }

    fn boxed<'a, T: Send + 'a>(iter: impl Iterator<Item = T> + Send + 'a) -> BoxedIterator<'a, T> {
        Box::new(iter)
    }

    #[test]
    fn test_extend_sorted_vec() {
        let mut target = vec![(1, "a"), (3, "c")];
        let other = vec![(2, "b"), (3, "c_new")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (3, "c_new")]);
    }

    #[test]
    fn test_extend_sorted_vec_empty_target() {
        let mut target: Vec<(i32, &str)> = vec![];
        let other = vec![(1, "a"), (2, "b")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b")]);
    }

    #[test]
    fn test_extend_sorted_vec_empty_other() {
        let mut target = vec![(1, "a"), (2, "b")];
        let other: Vec<(i32, &str)> = vec![];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b")]);
    }

    #[test]
    fn test_extend_sorted_vec_all_duplicates() {
        let mut target = vec![(1, "old1"), (2, "old2"), (3, "old3")];
        let other = vec![(1, "new1"), (2, "new2"), (3, "new3")];
        extend_sorted_vec(&mut target, &other);
        // other takes precedence
        assert_eq!(target, vec![(1, "new1"), (2, "new2"), (3, "new3")]);
    }

    #[test]
    fn test_extend_sorted_vec_interleaved() {
        let mut target = vec![(1, "a"), (3, "c"), (5, "e")];
        let other = vec![(2, "b"), (4, "d"), (6, "f")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (3, "c"), (4, "d"), (5, "e"), (6, "f")]);
    }

    #[test]
    fn test_extend_sorted_vec_other_all_smaller() {
        let mut target = vec![(5, "e"), (6, "f")];
        let other = vec![(1, "a"), (2, "b")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (5, "e"), (6, "f")]);
    }

    #[test]
    fn test_extend_sorted_vec_other_all_larger() {
        let mut target = vec![(1, "a"), (2, "b")];
        let other = vec![(5, "e"), (6, "f")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (5, "e"), (6, "f")]);
    }

    #[test]
    fn test_kway_merge_sorted_basic() {
        let slice1 = [(1, "a1"), (3, "c1")];
        let slice2 = [(2, "b2"), (3, "c2")];
        let slice3 = [(1, "a3"), (4, "d3")];

        let result = kway_merge_sorted([
            boxed(slice1.iter().copied()),
            boxed(slice2.iter().copied()),
            boxed(slice3.iter().copied()),
        ])
        .collect::<Vec<_>>();
        // First occurrence wins: key 1 -> a1 (slice1), key 3 -> c1 (slice1)
        assert_eq!(result, vec![(1, "a1"), (2, "b2"), (3, "c1"), (4, "d3")]);
    }

    #[test]
    fn test_kway_merge_sorted_empty_slices() {
        let slice1: [(i32, &str); 0] = [];
        let slice2 = [(1, "a")];
        let slice3: [(i32, &str); 0] = [];

        let result = kway_merge_sorted([
            boxed(slice1.iter().copied()),
            boxed(slice2.iter().copied()),
            boxed(slice3.iter().copied()),
        ])
        .collect::<Vec<_>>();
        assert_eq!(result, vec![(1, "a")]);
    }

    #[test]
    fn test_kway_merge_sorted_all_same_key() {
        let slice1 = [(5, "first")];
        let slice2 = [(5, "middle")];
        let slice3 = [(5, "last")];

        let result = kway_merge_sorted([
            boxed(slice1.iter().copied()),
            boxed(slice2.iter().copied()),
            boxed(slice3.iter().copied()),
        ])
        .collect::<Vec<_>>();
        // First occurrence wins (slice1 has highest priority)
        assert_eq!(result, vec![(5, "first")]);
    }

    #[test]
    fn test_kway_merge_sorted_single_slice() {
        let slice = [(1, "a"), (2, "b"), (3, "c")];
        let result = kway_merge_sorted([boxed(slice.iter().copied())]).collect::<Vec<_>>();
        assert_eq!(result, vec![(1, "a"), (2, "b"), (3, "c")]);
    }

    #[test]
    fn test_kway_merge_sorted_no_slices() {
        let result: Vec<(i32, &str)> =
            kway_merge_sorted(Vec::<BoxedIterator<'_, (i32, &str)>>::new()).collect();
        assert!(result.is_empty());
    }

    #[test]
    fn test_kway_merge_sorted_clones_lazily() {
        let clone_count = Arc::new(AtomicUsize::new(0));
        let slice = [
            (1, CloneCounter { value: 1, count: Arc::clone(&clone_count) }),
            (2, CloneCounter { value: 2, count: Arc::clone(&clone_count) }),
            (3, CloneCounter { value: 3, count: Arc::clone(&clone_count) }),
        ];

        let mut result = kway_merge_sorted([boxed(slice.iter().cloned())]);
        assert_eq!(clone_count.load(Ordering::Relaxed), 0);
        assert_eq!(result.next().unwrap().1.value, 1);
        assert_eq!(clone_count.load(Ordering::Relaxed), 1);
    }
}
