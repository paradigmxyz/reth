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
    // Fast paths for empty inputs
    if base.is_empty() {
        return overlay.to_vec();
    }
    if overlay.is_empty() {
        return base.to_vec();
    }

    let mut result = Vec::with_capacity(base.len() + overlay.len());
    let mut base_idx = 0;
    let mut overlay_idx = 0;

    // Merge while both have elements
    while base_idx < base.len() && overlay_idx < overlay.len() {
        match base[base_idx].0.cmp(&overlay[overlay_idx].0) {
            Ordering::Less => {
                result.push(base[base_idx].clone());
                base_idx += 1;
            }
            Ordering::Greater => {
                result.push(overlay[overlay_idx].clone());
                overlay_idx += 1;
            }
            Ordering::Equal => {
                // Overlay takes precedence, skip base
                result.push(overlay[overlay_idx].clone());
                base_idx += 1;
                overlay_idx += 1;
            }
        }
    }

    // Batch extend remaining elements (more efficient than one-by-one)
    if base_idx < base.len() {
        result.extend(base[base_idx..].iter().cloned());
    } else if overlay_idx < overlay.len() {
        result.extend(overlay[overlay_idx..].iter().cloned());
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
