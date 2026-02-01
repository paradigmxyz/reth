//! Extension trait for [`TrieMask`] providing efficient iteration over set bits.

use alloy_trie::TrieMask;

/// Extension trait for [`TrieMask`] that provides efficient iteration over set bits.
///
/// This is more efficient than iterating over `CHILD_INDEX_RANGE` and checking `is_bit_set`
/// for each index, as it directly iterates only the set bits using bit manipulation.
pub trait TrieMaskExt {
    /// Returns an iterator over the indices of set bits in the mask.
    ///
    /// The iterator yields values in ascending order (0 to 15). Use `.rev()` for descending order.
    fn iter_set_bits(&self) -> TrieMaskIter;
}

impl TrieMaskExt for TrieMask {
    #[inline]
    fn iter_set_bits(&self) -> TrieMaskIter {
        TrieMaskIter { mask: self.get() }
    }
}

/// An iterator over the set bit indices of a [`TrieMask`].
///
/// Iterates in ascending order by default. Use `.rev()` for descending order.
#[derive(Debug, Clone)]
pub struct TrieMaskIter {
    mask: u16,
}

impl Iterator for TrieMaskIter {
    type Item = u8;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.mask == 0 {
            return None;
        }
        let bit = self.mask.trailing_zeros() as u8;
        self.mask &= self.mask - 1; // Clear the lowest set bit
        Some(bit)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = self.mask.count_ones() as usize;
        (count, Some(count))
    }
}

impl ExactSizeIterator for TrieMaskIter {}

impl DoubleEndedIterator for TrieMaskIter {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.mask == 0 {
            return None;
        }
        let bit = 15 - self.mask.leading_zeros() as u8;
        self.mask &= !(1 << bit); // Clear the highest set bit
        Some(bit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_mask() {
        let mask = TrieMask::new(0);
        assert_eq!(mask.iter_set_bits().collect::<Vec<_>>(), Vec::<u8>::new());
        assert_eq!(mask.iter_set_bits().rev().collect::<Vec<_>>(), Vec::<u8>::new());
    }

    #[test]
    fn test_single_bit() {
        let mask = TrieMask::new(0b0000_0000_0000_0001); // bit 0
        assert_eq!(mask.iter_set_bits().collect::<Vec<_>>(), vec![0]);
        assert_eq!(mask.iter_set_bits().rev().collect::<Vec<_>>(), vec![0]);

        let mask = TrieMask::new(0b1000_0000_0000_0000); // bit 15
        assert_eq!(mask.iter_set_bits().collect::<Vec<_>>(), vec![15]);
        assert_eq!(mask.iter_set_bits().rev().collect::<Vec<_>>(), vec![15]);
    }

    #[test]
    fn test_multiple_bits() {
        let mask = TrieMask::new(0b0000_0000_0010_0101); // bits 0, 2, 5
        assert_eq!(mask.iter_set_bits().collect::<Vec<_>>(), vec![0, 2, 5]);
        assert_eq!(mask.iter_set_bits().rev().collect::<Vec<_>>(), vec![5, 2, 0]);
    }

    #[test]
    fn test_all_bits() {
        let mask = TrieMask::new(0xFFFF);
        assert_eq!(mask.iter_set_bits().collect::<Vec<_>>(), (0..16).collect::<Vec<_>>());
        assert_eq!(
            mask.iter_set_bits().rev().collect::<Vec<_>>(),
            (0..16).rev().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_exact_size() {
        let mask = TrieMask::new(0b0000_0000_0010_0101); // bits 0, 2, 5
        assert_eq!(mask.iter_set_bits().len(), 3);
        assert_eq!(mask.iter_set_bits().rev().len(), 3);
    }

    #[test]
    fn test_double_ended() {
        let mask = TrieMask::new(0b0000_0000_0010_0101); // bits 0, 2, 5
        let mut iter = mask.iter_set_bits();
        assert_eq!(iter.next(), Some(0));
        assert_eq!(iter.next_back(), Some(5));
        assert_eq!(iter.next(), Some(2));
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
    }
}
