use alloc::vec::Vec;
use core::cmp::Ordering;

/// Merges two sorted key-value vectors using a linear-time two-pointer algorithm.
///
/// Both `target` and `other` must be sorted by key. For duplicate keys, `other` takes
/// precedence (conceptually appended after `target`). Within each slice, later entries
/// override earlier ones. The result replaces `target` with the merged, deduplicated vector.
pub(crate) fn extend_sorted_vec<K, V>(target: &mut Vec<(K, V)>, other: &[(K, V)])
where
    K: Clone + Ord,
    V: Clone,
{
    if other.is_empty() {
        return;
    }
    if target.is_empty() {
        target.extend(other.iter().cloned());
        dedup_sorted_by_key(target);
        return;
    }

    let mut merged = Vec::with_capacity(target.len() + other.len());

    let mut target_idx = 0;
    let mut other_idx = 0;

    // Two-pointer merge: compare heads of both slices, take the smaller key.
    while target_idx < target.len() && other_idx < other.len() {
        let target_key = &target[target_idx].0;
        let other_key = &other[other_idx].0;

        match target_key.cmp(other_key) {
            Ordering::Less => {
                push_or_update(&mut merged, &target[target_idx]);
                target_idx += 1;
            }
            Ordering::Greater => {
                push_or_update(&mut merged, &other[other_idx]);
                other_idx += 1;
            }
            Ordering::Equal => {
                // Keys match: push target first, other will overwrite next iteration.
                // We only advance target_idx; other_idx stays so other's value overwrites.
                push_or_update(&mut merged, &target[target_idx]);
                target_idx += 1;
            }
        }
    }

    while target_idx < target.len() {
        push_or_update(&mut merged, &target[target_idx]);
        target_idx += 1;
    }

    while other_idx < other.len() {
        push_or_update(&mut merged, &other[other_idx]);
        other_idx += 1;
    }

    *target = merged;
}

/// Appends an entry to `merged`, collapsing consecutive duplicates.
///
/// If the new entry's key matches the last key in `merged`, the value is
/// overwritten instead of pushing a new entry. This is correct because:
/// 1. Both input slices are sorted, so duplicates are always consecutive
/// 2. We process entries in sorted order during the merge
#[inline]
fn push_or_update<K: Clone + Eq, V: Clone>(merged: &mut Vec<(K, V)>, entry: &(K, V)) {
    if let Some((last_key, last_value)) = merged.last_mut()
        && *last_key == entry.0
    {
        // Duplicate key: overwrite value (last one wins)
        *last_value = entry.1.clone();
        return;
    }
    merged.push((entry.0.clone(), entry.1.clone()));
}

/// Deduplicates a sorted vector in-place, keeping the last value for each key.
///
/// Uses a classic compaction pattern with read/write pointers. Runs in O(n) time
/// with O(1) extra space.
#[inline]
fn dedup_sorted_by_key<K: Eq, V>(vec: &mut Vec<(K, V)>) {
    if vec.len() <= 1 {
        return;
    }

    let mut write_idx = 0;
    for read_idx in 1..vec.len() {
        if vec[write_idx].0 == vec[read_idx].0 {
            vec.swap(write_idx, read_idx);
        } else {
            write_idx += 1;
            if write_idx != read_idx {
                vec.swap(write_idx, read_idx);
            }
        }
    }
    vec.truncate(write_idx + 1);
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
    fn test_disjoint_no_duplicates() {
        let mut target = vec![(1, "a"), (3, "c")];
        let other = vec![(2, "b"), (4, "d")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (3, "c"), (4, "d")]);
    }

    #[test]
    fn test_duplicates_in_target_later_wins() {
        let mut target = vec![(1, "a"), (1, "a2"), (2, "b")];
        let other: Vec<(i32, &str)> = vec![];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (1, "a2"), (2, "b")]);
    }

    #[test]
    fn test_duplicates_in_other_later_wins() {
        let mut target = vec![(1, "a")];
        let other = vec![(1, "b"), (1, "b2")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "b2")]);
    }

    #[test]
    fn test_duplicates_both_sides_other_wins() {
        let mut target = vec![(1, "a"), (1, "a2")];
        let other = vec![(1, "b"), (1, "b2")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "b2")]);
    }

    #[test]
    fn test_empty_target() {
        let mut target: Vec<(i32, &str)> = vec![];
        let other = vec![(1, "a")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a")]);
    }

    #[test]
    fn test_both_empty() {
        let mut target: Vec<(i32, &str)> = vec![];
        let other: Vec<(i32, &str)> = vec![];
        extend_sorted_vec(&mut target, &other);
        assert!(target.is_empty());
    }

    #[test]
    fn test_other_all_before_target() {
        let mut target = vec![(5, "e"), (6, "f")];
        let other = vec![(1, "a"), (2, "b")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (5, "e"), (6, "f")]);
    }

    #[test]
    fn test_other_all_after_target() {
        let mut target = vec![(1, "a"), (2, "b")];
        let other = vec![(5, "e"), (6, "f")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (5, "e"), (6, "f")]);
    }

    #[test]
    fn test_interleaved_with_overlaps() {
        let mut target = vec![(1, "a"), (3, "c"), (5, "e"), (7, "g")];
        let other = vec![(2, "b"), (3, "c_new"), (4, "d"), (7, "g_new"), (8, "h")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(
            target,
            vec![(1, "a"), (2, "b"), (3, "c_new"), (4, "d"), (5, "e"), (7, "g_new"), (8, "h")]
        );
    }

    #[test]
    fn test_empty_other_with_duplicates_in_target() {
        let mut target = vec![(1, "a"), (1, "a2")];
        let other: Vec<(i32, &str)> = vec![];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (1, "a2")]);
    }

    #[test]
    fn test_dedup_sorted_by_key() {
        let mut vec = vec![(1, "a"), (1, "b"), (2, "c"), (2, "d"), (2, "e"), (3, "f")];
        dedup_sorted_by_key(&mut vec);
        assert_eq!(vec, vec![(1, "b"), (2, "e"), (3, "f")]);
    }

    #[test]
    fn test_dedup_sorted_by_key_no_duplicates() {
        let mut vec = vec![(1, "a"), (2, "b"), (3, "c")];
        dedup_sorted_by_key(&mut vec);
        assert_eq!(vec, vec![(1, "a"), (2, "b"), (3, "c")]);
    }

    #[test]
    fn test_dedup_sorted_by_key_all_same() {
        let mut vec = vec![(1, "a"), (1, "b"), (1, "c")];
        dedup_sorted_by_key(&mut vec);
        assert_eq!(vec, vec![(1, "c")]);
    }

    #[test]
    fn test_empty_target_with_duplicates_in_other() {
        let mut target: Vec<(i32, &str)> = vec![];
        let other = vec![(1, "a"), (1, "b"), (2, "c")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "b"), (2, "c")]);
    }
}
