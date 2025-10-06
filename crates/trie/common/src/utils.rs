use alloc::vec::Vec;

/// Helper function to extend a sorted vector with another sorted vector.
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
    K: Clone + Ord + core::hash::Hash + Eq,
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
            use core::cmp::Ordering;
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
