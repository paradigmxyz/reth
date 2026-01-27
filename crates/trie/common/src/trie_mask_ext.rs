//! Extension trait for [`TrieMask`] providing efficient iteration.

use alloy_trie::TrieMask;

/// Extension trait for [`TrieMask`] providing efficient bit iteration.
pub trait TrieMaskExt {
    /// Returns an iterator over the indices of set bits (0-15).
    /// More efficient than scanning all 16 nibbles when the mask is sparse.
    fn iter_set_bits(self) -> SetBitsIter;
}

impl TrieMaskExt for TrieMask {
    #[inline]
    fn iter_set_bits(self) -> SetBitsIter {
        SetBitsIter { mask: self.get() }
    }
}

/// Iterator over set bit indices in a [`TrieMask`].
#[derive(Debug, Clone)]
pub struct SetBitsIter {
    mask: u16,
}

impl Iterator for SetBitsIter {
    type Item = u8;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.mask == 0 {
            return None;
        }
        let idx = self.mask.trailing_zeros() as u8;
        self.mask &= self.mask - 1; // clear lowest set bit
        Some(idx)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = self.mask.count_ones() as usize;
        (count, Some(count))
    }
}

impl ExactSizeIterator for SetBitsIter {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iter_set_bits() {
        let mask = TrieMask::new(0b1010_0101); // bits 0, 2, 5, 7 set
        let bits: Vec<u8> = mask.iter_set_bits().collect();
        assert_eq!(bits, vec![0, 2, 5, 7]);

        let empty = TrieMask::new(0);
        assert_eq!(empty.iter_set_bits().count(), 0);

        let full = TrieMask::new(0xFFFF);
        assert_eq!(full.iter_set_bits().count(), 16);
    }

    #[test]
    fn test_iter_set_bits_exact_size() {
        let mask = TrieMask::new(0b1010_0101);
        let iter = mask.iter_set_bits();
        assert_eq!(iter.len(), 4);
    }

    #[test]
    fn test_iter_set_bits_single() {
        for i in 0..16u8 {
            let mask = TrieMask::new(1 << i);
            let bits: Vec<u8> = mask.iter_set_bits().collect();
            assert_eq!(bits, vec![i]);
        }
    }
}
