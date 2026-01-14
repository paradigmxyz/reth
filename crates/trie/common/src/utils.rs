use alloc::vec::Vec;
use core::cmp::Ordering;
use itertools::Itertools;

/// Merge sorted slices into sorted iterator. First occurrence wins for duplicate keys.
///
/// Callers pass slices in priority order (index 0 = highest priority), so the first
/// slice's value for a key takes precedence over later slices.
pub(crate) fn kway_merge_sorted<'a, K, V>(
    slices: impl IntoIterator<Item = &'a [(K, V)]>,
) -> impl Iterator<Item = (K, V)>
where
    K: Ord + Clone + 'a,
    V: Clone + 'a,
{
    slices
        .into_iter()
        .filter(|s| !s.is_empty())
        .enumerate()
        .map(|(i, s)| s.iter().cloned().map(move |item| (i, item)))
        .kmerge_by(|(i1, a), (i2, b)| (&a.0, *i1) < (&b.0, *i2))
        .coalesce(|(i1, a), (i2, b)| if a.0 == b.0 { Ok((i1, a)) } else { Err(((i1, a), (i2, b))) })
        .map(|(_, item)| item)
        .collect::<Vec<_>>()
        .into_iter()
}

/// Extend a sorted vector with another sorted vector.
/// Values from `other` take precedence for duplicate keys.
///
/// This function efficiently merges two sorted vectors by:
/// 1. Iterating through the target vector with mutable references
/// 2. Using a peekable iterator for the other vector
/// 3. For each target item, processing other items that come before or equal to it
/// 4. Collecting items from other that need to be inserted
/// 5. Appending and re-sorting only if new items were added
pub(crate) fn extend_sorted_vec<K, V>(target: &mut Vec<(K, V)>, other: &[(K, V)])
where
    K: Clone + Ord,
    V: Clone,
{
    if other.is_empty() {
        return;
    }

    let mut other_iter = other.iter().peekable();
    let mut to_insert = Vec::new();

    // Iterate through target and update/collect items from other
    for target_item in target.iter_mut() {
        while let Some(other_item) = other_iter.peek() {
            match other_item.0.cmp(&target_item.0) {
                Ordering::Less => {
                    // Other item comes before current target item, collect it
                    to_insert.push(other_iter.next().unwrap().clone());
                }
                Ordering::Equal => {
                    // Same key, update target with other's value
                    target_item.1 = other_iter.next().unwrap().1.clone();
                    break;
                }
                Ordering::Greater => {
                    // Other item comes after current target item, keep target unchanged
                    break;
                }
            }
        }
    }

    // Append collected new items, as well as any remaining from `other` which are necessarily also
    // new, and sort if needed
    if !to_insert.is_empty() || other_iter.peek().is_some() {
        target.extend(to_insert);
        target.extend(other_iter.cloned());
        target.sort_unstable_by(|a, b| a.0.cmp(&b.0));
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

        let result: Vec<_> =
            kway_merge_sorted([slice1.as_slice(), slice2.as_slice(), slice3.as_slice()]).collect();
        // First occurrence wins: key 1 -> a1 (slice1), key 3 -> c1 (slice1)
        assert_eq!(result, vec![(1, "a1"), (2, "b2"), (3, "c1"), (4, "d3")]);
    }

    #[test]
    fn test_kway_merge_sorted_empty_slices() {
        let slice1: Vec<(i32, &str)> = vec![];
        let slice2 = vec![(1, "a")];
        let slice3: Vec<(i32, &str)> = vec![];

        let result: Vec<_> =
            kway_merge_sorted([slice1.as_slice(), slice2.as_slice(), slice3.as_slice()]).collect();
        assert_eq!(result, vec![(1, "a")]);
    }

    #[test]
    fn test_kway_merge_sorted_all_same_key() {
        let slice1 = vec![(5, "first")];
        let slice2 = vec![(5, "middle")];
        let slice3 = vec![(5, "last")];

        let result: Vec<_> =
            kway_merge_sorted([slice1.as_slice(), slice2.as_slice(), slice3.as_slice()]).collect();
        // First occurrence wins (slice1 has highest priority)
        assert_eq!(result, vec![(5, "first")]);
    }

    #[test]
    fn test_kway_merge_sorted_single_slice() {
        let slice = vec![(1, "a"), (2, "b"), (3, "c")];
        let result: Vec<_> = kway_merge_sorted([slice.as_slice()]).collect();
        assert_eq!(result, vec![(1, "a"), (2, "b"), (3, "c")]);
    }

    #[test]
    fn test_kway_merge_sorted_no_slices() {
        let result: Vec<(i32, &str)> = kway_merge_sorted(Vec::<&[(i32, &str)]>::new()).collect();
        assert!(result.is_empty());
    }
}
