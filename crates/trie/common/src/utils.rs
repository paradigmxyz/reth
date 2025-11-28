use alloc::vec::Vec;
use itertools::Itertools;

/// Helper function to extend a sorted vector with another sorted vector.
/// Values from `other` take precedence for duplicate keys.
///
/// **Both `target` and `other` MUST be sorted by key** for this function to work correctly.
/// If not, the result will be incorrect and may not maintain sorted order.
///
/// This function efficiently merges two sorted vectors using `itertools::merge_join_by`:
/// 1. Merges in one pass (O(n+m)) by comparing keys
/// 2. For duplicate keys, `other` values take precedence
pub(crate) fn extend_sorted_vec<K, V>(target: &mut Vec<(K, V)>, other: &[(K, V)])
where
    K: Clone + Ord,
    V: Clone,
{
    if other.is_empty() {
        return;
    }

    *target = target
        .iter()
        .merge_join_by(other.iter(), |(k1, _), (k2, _)| k1.cmp(k2))
        .map(|entry| {
            // Extract the item: for duplicates, use other's value
            match entry {
                itertools::EitherOrBoth::Left(item) |
                itertools::EitherOrBoth::Right(item) |
                itertools::EitherOrBoth::Both(_, item) => item.clone(),
            }
        })
        .collect();
}
