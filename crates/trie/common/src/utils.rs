use alloc::vec::Vec;
use core::cmp::Ordering;
use itertools::Itertools;

/// Merge sorted iterators into a sorted iterator. First occurrence wins for duplicate keys.
///
/// Callers pass iterators in priority order (index 0 = highest priority), so the first
/// iterator's value for a key takes precedence over later iterators.
pub(crate) fn kway_merge_sorted<K, V, I, J>(iterators: I) -> impl Iterator<Item = (K, V)>
where
    K: Ord,
    I: IntoIterator<Item = J>,
    J: IntoIterator<Item = (K, V)>,
{
    iterators
        .into_iter()
        .enumerate()
        .map(|(i, iter)| iter.into_iter().map(move |(k, v)| (i, k, v)))
        .kmerge_by(|(i1, k1, _), (i2, k2, _)| (k1, i1) < (k2, i2))
        .dedup_by(|(_, k1, _), (_, k2, _)| k1 == k2)
        .map(|(_, k, v)| (k, v))
}

/// Merge sorted left slices, excluding keys present in any right slice unless one of the right
/// values is equal to the selected left value.
/// Retained keys and values are cloned as the returned iterator is consumed.
///
/// Callers pass left slices in priority order (index 0 = highest priority), so the first
/// left slice's value for a key takes precedence over later slices. Right slice order is ignored.
pub(crate) fn kway_merge_disjoint_sorted<'a, K, V>(
    left_slices: impl IntoIterator<Item = &'a [(K, V)]>,
    right_slices: impl IntoIterator<Item = &'a [(K, V)]>,
) -> impl Iterator<Item = (K, V)>
where
    K: Ord + Clone + 'a,
    V: Clone + PartialEq + 'a,
{
    let mut right_entries = right_slices
        .into_iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.iter())
        .kmerge_by(|(left_key, _), (right_key, _)| left_key < right_key)
        .peekable();

    left_slices
        .into_iter()
        .filter(|s| !s.is_empty())
        .enumerate()
        .map(|(i, s)| s.iter().map(move |(k, v)| (i, k, v)))
        .kmerge_by(|(i1, k1, _), (i2, k2, _)| (k1, i1) < (k2, i2))
        .dedup_by(|(_, k1, _), (_, k2, _)| *k1 == *k2)
        .filter_map(move |(_, key, value)| {
            while let Some((right_key, _)) = right_entries.peek().copied() {
                if right_key >= key {
                    break
                }
                right_entries.next();
            }

            let mut has_mask = false;
            let mut has_equal_mask = false;
            while let Some((right_key, right_value)) = right_entries.peek().copied() {
                if right_key != key {
                    break
                }

                has_mask = true;
                if !has_equal_mask {
                    has_equal_mask = right_value == value;
                }
                right_entries.next();
            }

            if has_mask && !has_equal_mask {
                return None
            }

            Some((key.clone(), value.clone()))
        })
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
    use alloc::rc::Rc;
    use core::cell::Cell;

    #[derive(Debug)]
    struct CloneCounter {
        value: u8,
        count: Rc<Cell<usize>>,
    }

    impl Clone for CloneCounter {
        fn clone(&self) -> Self {
            self.count.set(self.count.get() + 1);
            Self { value: self.value, count: Rc::clone(&self.count) }
        }
    }

    impl PartialEq for CloneCounter {
        fn eq(&self, other: &Self) -> bool {
            self.value == other.value
        }
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
        let slice1 = vec![(1, "a1"), (3, "c1")];
        let slice2 = vec![(2, "b2"), (3, "c2")];
        let slice3 = vec![(1, "a3"), (4, "d3")];

        let result = kway_merge_sorted([slice1, slice2, slice3]).collect::<Vec<_>>();
        // First occurrence wins: key 1 -> a1 (slice1), key 3 -> c1 (slice1)
        assert_eq!(result, vec![(1, "a1"), (2, "b2"), (3, "c1"), (4, "d3")]);
    }

    #[test]
    fn test_kway_merge_sorted_empty_slices() {
        let slice1: Vec<(i32, &str)> = vec![];
        let slice2 = vec![(1, "a")];
        let slice3: Vec<(i32, &str)> = vec![];

        let result = kway_merge_sorted([slice1, slice2, slice3]).collect::<Vec<_>>();
        assert_eq!(result, vec![(1, "a")]);
    }

    #[test]
    fn test_kway_merge_sorted_all_same_key() {
        let slice1 = vec![(5, "first")];
        let slice2 = vec![(5, "middle")];
        let slice3 = vec![(5, "last")];

        let result = kway_merge_sorted([slice1, slice2, slice3]).collect::<Vec<_>>();
        // First occurrence wins (slice1 has highest priority)
        assert_eq!(result, vec![(5, "first")]);
    }

    #[test]
    fn test_kway_merge_sorted_single_slice() {
        let slice = vec![(1, "a"), (2, "b"), (3, "c")];
        let result = kway_merge_sorted([slice]).collect::<Vec<_>>();
        assert_eq!(result, vec![(1, "a"), (2, "b"), (3, "c")]);
    }

    #[test]
    fn test_kway_merge_sorted_no_slices() {
        let result = kway_merge_sorted(Vec::<Vec<(i32, &str)>>::new()).collect::<Vec<_>>();
        assert!(result.is_empty());
    }

    #[test]
    fn test_kway_merge_disjoint_sorted() {
        let left_old = vec![(1, "old"), (2, "drop"), (4, "keep")];
        let left_new = vec![(1, "new"), (3, "new_only")];
        let right_a = vec![(2, "ignored"), (5, "ignored")];
        let right_b = vec![(3, "ignored")];

        let result = kway_merge_disjoint_sorted(
            [left_new.as_slice(), left_old.as_slice()],
            [right_a.as_slice(), right_b.as_slice()],
        )
        .collect::<Vec<_>>();

        assert_eq!(result, vec![(1, "new"), (4, "keep")]);
    }

    #[test]
    fn test_kway_merge_disjoint_sorted_keeps_equal_overlaps() {
        let left = vec![(1, "equal"), (2, "equal"), (3, "drop")];
        let right_a = vec![(1, "different"), (2, "equal"), (3, "different")];
        let right_b = vec![(1, "equal"), (2, "different")];

        let result =
            kway_merge_disjoint_sorted([left.as_slice()], [right_a.as_slice(), right_b.as_slice()])
                .collect::<Vec<_>>();
        let reversed =
            kway_merge_disjoint_sorted([left.as_slice()], [right_b.as_slice(), right_a.as_slice()])
                .collect::<Vec<_>>();

        assert_eq!(result, vec![(1, "equal"), (2, "equal")]);
        assert_eq!(reversed, result);
    }

    #[test]
    fn test_kway_merge_disjoint_sorted_clones_lazily() {
        let clone_count = Rc::new(Cell::new(0));
        let left = vec![
            (1, CloneCounter { value: 1, count: Rc::clone(&clone_count) }),
            (2, CloneCounter { value: 2, count: Rc::clone(&clone_count) }),
            (3, CloneCounter { value: 3, count: Rc::clone(&clone_count) }),
        ];
        let mut result = kway_merge_disjoint_sorted(
            [left.as_slice()],
            core::iter::empty::<&[(i32, CloneCounter)]>(),
        );

        assert_eq!(clone_count.get(), 0);
        assert_eq!(result.next().map(|(key, _)| key), Some(1));
        assert_eq!(clone_count.get(), 1);
        drop(result);
        assert_eq!(clone_count.get(), 1);
    }
}
