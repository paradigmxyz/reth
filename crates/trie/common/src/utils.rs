use alloc::vec::Vec;
use core::cmp::Ordering;

/// Helper function to extend a sorted vector with another sorted vector.
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
}
