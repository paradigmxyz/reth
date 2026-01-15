use alloc::vec::Vec;
use core::cmp::Ordering;
use itertools::Itertools;

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

/// Extend a sorted vector with another sorted vector.
/// Values from `other` take precedence for duplicate keys.
///
/// Values from `other` take precedence for duplicate keys.
pub(crate) fn extend_sorted_vec<K, V>(target: &mut Vec<(K, V)>, other: &[(K, V)])
where
    K: Clone + Ord,
    V: Clone,
{
    let cmp = |a: &(K, V), b: &(K, V)| a.0.cmp(&b.0);

    if other.is_empty() {
        return;
    }
    target.reserve(other.len());

    let mut other_iter = other.iter().peekable();
    let initial_len = target.len();
    for i in 0..initial_len {
        while let Some(other_item) = other_iter.peek() {
            let target_item = &mut target[i];
            match cmp(other_item, target_item) {
                Ordering::Less => {
                    target.push(other_iter.next().unwrap().clone());
                }
                Ordering::Equal => {
                    target_item.1 = other_iter.next().unwrap().1.clone();
                    break;
                }
                Ordering::Greater => {
                    break;
                }
            }
        }
    }

    target.extend(other_iter.cloned());
    if target.len() > initial_len {
        target.sort_by(cmp);
    }
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
}
