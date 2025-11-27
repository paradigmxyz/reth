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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extend_sorted_vec_empty_other() {
        let mut target = vec![(1, "a"), (2, "b"), (3, "c")];
        let other: Vec<(i32, &str)> = vec![];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (3, "c")]);
    }

    #[test]
    fn test_extend_sorted_vec_empty_target() {
        let mut target: Vec<(i32, &str)> = vec![];
        let other = vec![(1, "a"), (2, "b"), (3, "c")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (3, "c")]);
    }

    #[test]
    fn test_extend_sorted_vec_no_overlap() {
        let mut target = vec![(1, "a"), (3, "c"), (5, "e")];
        let other = vec![(2, "b"), (4, "d"), (6, "f")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (3, "c"), (4, "d"), (5, "e"), (6, "f")]);
    }

    #[test]
    fn test_extend_sorted_vec_with_duplicates_other_precedence() {
        let mut target = vec![(1, "a"), (2, "target"), (3, "c")];
        let other = vec![(2, "other"), (4, "d")];
        extend_sorted_vec(&mut target, &other);
        // other's value should take precedence for key 2
        assert_eq!(target, vec![(1, "a"), (2, "other"), (3, "c"), (4, "d")]);
    }

    #[test]
    fn test_extend_sorted_vec_all_other_before_target() {
        let mut target = vec![(5, "e"), (6, "f")];
        let other = vec![(1, "a"), (2, "b"), (3, "c")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (3, "c"), (5, "e"), (6, "f")]);
    }

    #[test]
    fn test_extend_sorted_vec_all_other_after_target() {
        let mut target = vec![(1, "a"), (2, "b")];
        let other = vec![(5, "e"), (6, "f"), (7, "g")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (5, "e"), (6, "f"), (7, "g")]);
    }

    #[test]
    fn test_extend_sorted_vec_interleaved() {
        let mut target = vec![(1, "a"), (3, "c"), (5, "e"), (7, "g")];
        let other = vec![(2, "b"), (4, "d"), (6, "f")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(
            target,
            vec![(1, "a"), (2, "b"), (3, "c"), (4, "d"), (5, "e"), (6, "f"), (7, "g")]
        );
    }

    #[test]
    fn test_extend_sorted_vec_multiple_duplicates() {
        let mut target = vec![(1, "target1"), (2, "target2"), (3, "target3")];
        let other = vec![(1, "other1"), (2, "other2"), (3, "other3")];
        extend_sorted_vec(&mut target, &other);
        // All other values should take precedence
        assert_eq!(target, vec![(1, "other1"), (2, "other2"), (3, "other3")]);
    }

    #[test]
    fn test_extend_sorted_vec_single_element_each() {
        let mut target = vec![(5, "target")];
        let other = vec![(3, "other")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(3, "other"), (5, "target")]);
    }

    #[test]
    fn test_extend_sorted_vec_same_key_different_values() {
        let mut target = vec![(1, "original")];
        let other = vec![(1, "override")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "override")]);
    }

    #[test]
    fn test_extend_sorted_vec_large_merge() {
        let mut target: Vec<(i32, i32)> = (0..100).step_by(2).map(|i| (i, i)).collect();
        let other: Vec<(i32, i32)> = (1..100).step_by(2).map(|i| (i, i * 10)).collect();
        extend_sorted_vec(&mut target, &other);

        // Verify sorted order
        for i in 0..(target.len() - 1) {
            assert!(target[i].0 < target[i + 1].0);
        }

        // Verify all keys are present
        assert_eq!(target.len(), 100);

        // Verify other values took precedence for odd numbers
        assert_eq!(target[1].1, 10); // key 1 should have value 10 from other
    }
}
