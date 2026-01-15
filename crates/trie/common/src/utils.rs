use alloc::vec::Vec;
use core::cmp::Ordering;

/// Extend a sorted vector with another sorted vector using two-pointer merge.
///
/// Values from `other` take precedence for duplicate keys.
///
/// Time: O(n + m), Space: O(n + m) for the merged result.
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
        return;
    }

    // Two-pointer merge into a new vector (O(n + m) time and space)
    let mut result = Vec::with_capacity(target.len() + other.len());
    let mut target_iter = target.iter().peekable();
    let mut other_iter = other.iter().peekable();

    while let (Some(t), Some(o)) = (target_iter.peek(), other_iter.peek()) {
        match t.0.cmp(&o.0) {
            Ordering::Less => {
                result.push(target_iter.next().unwrap().clone());
            }
            Ordering::Greater => {
                result.push(other_iter.next().unwrap().clone());
            }
            Ordering::Equal => {
                // `other` takes precedence for duplicate keys
                target_iter.next();
                result.push(other_iter.next().unwrap().clone());
            }
        }
    }

    // Drain remaining elements from whichever iterator has leftovers
    result.extend(target_iter.cloned());
    result.extend(other_iter.cloned());

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
}
