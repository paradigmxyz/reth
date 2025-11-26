use alloc::vec::Vec;
use core::cmp::Ordering;

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
}
