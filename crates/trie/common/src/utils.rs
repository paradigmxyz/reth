use alloc::vec::Vec;
use core::cmp::Ordering;

/// Merges two sorted slices into a new sorted vector.
/// Values from `overlay` take precedence for duplicate keys.
///
/// This is an O(n + m) merge operation that maintains sorted order.
#[inline]
pub(crate) fn merge_sorted_vecs<K, V>(base: &[(K, V)], overlay: &[(K, V)]) -> Vec<(K, V)>
where
    K: Clone + Ord,
    V: Clone,
{
    let mut result = Vec::with_capacity(base.len() + overlay.len());
    let mut base_iter = base.iter().peekable();
    let mut overlay_iter = overlay.iter().peekable();

    loop {
        match (base_iter.peek(), overlay_iter.peek()) {
            (Some(base_item), Some(overlay_item)) => {
                match base_item.0.cmp(&overlay_item.0) {
                    Ordering::Less => {
                        result.push(base_iter.next().unwrap().clone());
                    }
                    Ordering::Greater => {
                        result.push(overlay_iter.next().unwrap().clone());
                    }
                    Ordering::Equal => {
                        // Overlay takes precedence, skip base
                        base_iter.next();
                        result.push(overlay_iter.next().unwrap().clone());
                    }
                }
            }
            (Some(_), None) => {
                result.push(base_iter.next().unwrap().clone());
            }
            (None, Some(_)) => {
                result.push(overlay_iter.next().unwrap().clone());
            }
            (None, None) => break,
        }
    }

    result
}

/// Helper function to extend a sorted vector with another sorted vector.
/// Values from `other` take precedence for duplicate keys.
///
/// This is an O(n + m) merge operation that maintains sorted order.
#[inline]
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

    // Use the merge function and replace target
    let merged = merge_sorted_vecs(target, other);
    *target = merged;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_sorted_vecs_basic() {
        let base = vec![(1, "a"), (3, "c"), (5, "e")];
        let overlay = vec![(2, "b"), (4, "d")];
        let result = merge_sorted_vecs(&base, &overlay);
        assert_eq!(result, vec![(1, "a"), (2, "b"), (3, "c"), (4, "d"), (5, "e")]);
    }

    #[test]
    fn test_merge_sorted_vecs_with_duplicates() {
        let base = vec![(1, "a"), (2, "b_base"), (3, "c")];
        let overlay = vec![(2, "b_overlay"), (4, "d")];
        let result = merge_sorted_vecs(&base, &overlay);
        // Overlay takes precedence for key 2
        assert_eq!(result, vec![(1, "a"), (2, "b_overlay"), (3, "c"), (4, "d")]);
    }

    #[test]
    fn test_merge_sorted_vecs_empty_base() {
        let base: Vec<(i32, &str)> = vec![];
        let overlay = vec![(1, "a"), (2, "b")];
        let result = merge_sorted_vecs(&base, &overlay);
        assert_eq!(result, vec![(1, "a"), (2, "b")]);
    }

    #[test]
    fn test_merge_sorted_vecs_empty_overlay() {
        let base = vec![(1, "a"), (2, "b")];
        let overlay: Vec<(i32, &str)> = vec![];
        let result = merge_sorted_vecs(&base, &overlay);
        assert_eq!(result, vec![(1, "a"), (2, "b")]);
    }

    #[test]
    fn test_extend_sorted_vec() {
        let mut target = vec![(1, "a"), (3, "c")];
        let other = vec![(2, "b"), (3, "c_new")];
        extend_sorted_vec(&mut target, &other);
        assert_eq!(target, vec![(1, "a"), (2, "b"), (3, "c_new")]);
    }
}
