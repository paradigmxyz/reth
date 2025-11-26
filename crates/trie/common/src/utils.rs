use alloc::vec::Vec;

/// Helper function to extend a sorted vector with another sorted vector.
/// Values from `other` take precedence for duplicate keys.
///
/// This function efficiently merges two sorted vectors by:
/// 1. Using peekable iterators for both vectors to compare without consuming
/// 2. Merging in one pass (O(n+m)) by always choosing the smaller element
/// 3. Building the result in a pre-allocated vector
/// 4. Replacing target with the merged result
pub(crate) fn extend_sorted_vec<K, V>(target: &mut Vec<(K, V)>, other: &[(K, V)])
where
    K: Clone + Ord,
    V: Clone,
{
    if other.is_empty() {
        return;
    }

    let mut result = Vec::with_capacity(target.len() + other.len());
    let mut target_iter = target.iter().peekable();
    let mut other_iter = other.iter().peekable();

    loop {
        match (target_iter.peek(), other_iter.peek()) {
            (Some(target_item), Some(other_item)) => {
                use core::cmp::Ordering;
                match other_item.0.cmp(&target_item.0) {
                    Ordering::Less => {
                        result.push(other_iter.next().unwrap().clone());
                    }
                    Ordering::Equal => {
                        // duplicate key, use other's value (takes precedence)
                        result.push(other_iter.next().unwrap().clone());
                        target_iter.next();
                    }
                    Ordering::Greater => {
                        result.push(target_iter.next().unwrap().clone());
                    }
                }
            }
            (Some(_), None) => {
                // Only target items remaining
                result.extend(target_iter.cloned());
                break;
            }
            (None, Some(_)) => {
                // Only other items remaining
                result.extend(other_iter.cloned());
                break;
            }
            (None, None) => {
                // Both exhausted
                break;
            }
        }
    }

    *target = result;
}
