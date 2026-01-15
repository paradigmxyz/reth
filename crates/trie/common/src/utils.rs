use alloc::vec::Vec;
use core::cmp::Ordering;
use itertools::Itertools;

/// Minimum average items per source to prefer pairwise sorted merge over HashMap merge.
pub(crate) const PAIRWISE_MIN_AVG_ITEMS: usize = 2000;

/// Minimum number of sources that triggers k-way merge instead of pairwise sorted merge.
pub(crate) const KWAY_MIN_SOURCES: usize = 30;

/// Returns true if pairwise sorted merge is preferred over HashMap merge.
/// Returns false if k >= KWAY_MIN_SOURCES (use kway) or avg items < threshold (use HashMap).
#[inline]
pub(crate) fn prefer_sorted_merge(num_sources: usize, total_items: usize) -> bool {
    if num_sources >= KWAY_MIN_SOURCES {
        return false;
    }
    total_items >= PAIRWISE_MIN_AVG_ITEMS.saturating_mul(num_sources)
}

/// Merge sorted slices into a sorted `Vec`. First occurrence wins for duplicate keys.
///
/// Callers pass slices in priority order (index 0 = highest priority), so the first
/// slice's value for a key takes precedence over later slices.
pub(crate) fn kway_merge_sorted<'a, K, V>(
    slices: impl IntoIterator<Item = &'a [(K, V)]>,
) -> Vec<(K, V)>
where
    K: Ord + Clone + 'a,
    V: Clone + 'a,
{
    slices
        .into_iter()
        .filter(|s| !s.is_empty())
        .enumerate()
        // Merge by reference: (priority, &K, &V) - avoids cloning all elements upfront
        .map(|(i, s)| s.iter().map(move |(k, v)| (i, k, v)))
        .kmerge_by(|(i1, k1, _), (i2, k2, _)| (k1, i1) < (k2, i2))
        .dedup_by(|(_, k1, _), (_, k2, _)| *k1 == *k2)
        // Clone only surviving elements after dedup
        .map(|(_, k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Extend a sorted vector with another sorted vector using O(n+m) merge.
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
        let slice1 = vec![(1, "a1"), (3, "c1")];
        let slice2 = vec![(2, "b2"), (3, "c2")];
        let slice3 = vec![(1, "a3"), (4, "d3")];

        let result = kway_merge_sorted([slice1.as_slice(), slice2.as_slice(), slice3.as_slice()]);
        // First occurrence wins: key 1 -> a1 (slice1), key 3 -> c1 (slice1)
        assert_eq!(result, vec![(1, "a1"), (2, "b2"), (3, "c1"), (4, "d3")]);
    }

    #[test]
    fn test_kway_merge_sorted_empty_slices() {
        let slice1: Vec<(i32, &str)> = vec![];
        let slice2 = vec![(1, "a")];
        let slice3: Vec<(i32, &str)> = vec![];

        let result = kway_merge_sorted([slice1.as_slice(), slice2.as_slice(), slice3.as_slice()]);
        assert_eq!(result, vec![(1, "a")]);
    }

    #[test]
    fn test_kway_merge_sorted_all_same_key() {
        let slice1 = vec![(5, "first")];
        let slice2 = vec![(5, "middle")];
        let slice3 = vec![(5, "last")];

        let result = kway_merge_sorted([slice1.as_slice(), slice2.as_slice(), slice3.as_slice()]);
        // First occurrence wins (slice1 has highest priority)
        assert_eq!(result, vec![(5, "first")]);
    }

    #[test]
    fn test_kway_merge_sorted_single_slice() {
        let slice = vec![(1, "a"), (2, "b"), (3, "c")];
        let result = kway_merge_sorted([slice.as_slice()]);
        assert_eq!(result, vec![(1, "a"), (2, "b"), (3, "c")]);
    }

    #[test]
    fn test_kway_merge_sorted_no_slices() {
        let result: Vec<(i32, &str)> = kway_merge_sorted(Vec::<&[(i32, &str)]>::new());
        assert!(result.is_empty());
    }

    #[test]
    fn test_prefer_sorted_merge_kway_threshold() {
        assert!(!prefer_sorted_merge(30, 100_000));
        assert!(!prefer_sorted_merge(50, 200_000));
        assert!(prefer_sorted_merge(29, 29 * PAIRWISE_MIN_AVG_ITEMS));
    }

    #[test]
    fn test_prefer_sorted_merge_pairwise_threshold() {
        assert!(prefer_sorted_merge(5, 5 * PAIRWISE_MIN_AVG_ITEMS));
        assert!(prefer_sorted_merge(10, 10 * PAIRWISE_MIN_AVG_ITEMS + 1));
        assert!(!prefer_sorted_merge(5, 5 * PAIRWISE_MIN_AVG_ITEMS - 1));
    }

    #[test]
    fn test_prefer_sorted_merge_small_data() {
        assert!(!prefer_sorted_merge(2, 100));
        assert!(!prefer_sorted_merge(5, 1000));
    }
}
