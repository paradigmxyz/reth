use core::{
    fmt,
    ops::{Shl, ShlAssign, Shr, ShrAssign},
};
use derive_more::{
    BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Deref, From, Not,
};

/// A struct representing a mask of 16 bits, used for Ethereum trie operations.
///
/// Masks in a trie are used to efficiently represent and manage information about the presence or
/// absence of certain elements, such as child nodes, within a trie. Masks are usually implemented
/// as bit vectors, where each bit represents the presence (1) or absence (0) of a corresponding
/// element.
#[derive(
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Deref,
    From,
    BitAndAssign,
    BitAnd,
    BitOr,
    BitOrAssign,
    BitXor,
    BitXorAssign,
    Not,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "arbitrary",
    derive(derive_arbitrary::Arbitrary, proptest_derive::Arbitrary)
)]
pub struct TrieMask(u16);

impl fmt::Debug for TrieMask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TrieMask({:016b})", self.0)
    }
}

impl TrieMask {
    /// The size of this mask in bits.
    pub const BITS: u32 = u16::BITS;

    /// Creates a new `TrieMask` from the given inner value.
    #[inline]
    pub const fn new(inner: u16) -> Self {
        Self(inner)
    }

    /// Returns the inner value of the `TrieMask`.
    #[inline]
    pub const fn get(self) -> u16 {
        self.0
    }

    /// Creates a new `TrieMask` from the given nibble.
    #[inline]
    pub const fn from_nibble(nibble: u8) -> Self {
        Self(1u16 << nibble)
    }

    /// Returns `true` if the current `TrieMask` is a subset of `other`.
    #[inline]
    pub fn is_subset_of(self, other: Self) -> bool {
        self & other == self
    }

    /// Returns `true` if a given bit is set in a mask.
    #[inline]
    pub const fn is_bit_set(self, index: u8) -> bool {
        self.0 & (1u16 << index) != 0
    }

    /// Returns the number of bits set in the mask.
    #[inline]
    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// Returns `true` if the mask is empty.
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns the number of bits set in the mask.
    #[inline]
    pub const fn count_bits(self) -> u8 {
        self.0.count_ones() as u8
    }

    /// Returns the index of the first bit set in the mask, or `None` if the mask is empty.
    #[inline]
    pub const fn first_set_bit_index(self) -> Option<u8> {
        if self.is_empty() {
            None
        } else {
            Some(self.0.trailing_zeros() as u8)
        }
    }

    /// Set bit at a specified index.
    #[inline]
    pub const fn set_bit(&mut self, index: u8) {
        self.0 |= 1u16 << index;
    }

    /// Unset bit at a specified index.
    #[inline]
    pub const fn unset_bit(&mut self, index: u8) {
        self.0 &= !(1u16 << index);
    }

    /// Returns an iterator over the indices of set bits in the mask.
    ///
    /// The iterator yields values in ascending order (0 to 15). Use [`.rev()`](Iterator::rev) for
    /// descending order.
    ///
    /// This is more efficient than iterating over `0..16` and checking
    /// [`is_bit_set`](Self::is_bit_set) for each index, as it directly iterates only the set
    /// bits using bit manipulation.
    #[inline]
    pub const fn iter(self) -> TrieMaskIter {
        TrieMaskIter { mask: self.0 }
    }
}

impl<T> Shl<T> for TrieMask
where
    u16: Shl<T, Output = u16>,
{
    type Output = Self;

    #[inline]
    fn shl(self, rhs: T) -> Self::Output {
        Self(self.0.shl(rhs))
    }
}

impl<T> ShlAssign<T> for TrieMask
where
    u16: ShlAssign<T>,
{
    #[inline]
    fn shl_assign(&mut self, rhs: T) {
        self.0.shl_assign(rhs);
    }
}

impl<T> Shr<T> for TrieMask
where
    u16: Shr<T, Output = u16>,
{
    type Output = Self;

    #[inline]
    fn shr(self, rhs: T) -> Self::Output {
        Self(self.0.shr(rhs))
    }
}

impl<T> ShrAssign<T> for TrieMask
where
    u16: ShrAssign<T>,
{
    #[inline]
    fn shr_assign(&mut self, rhs: T) {
        self.0.shr_assign(rhs);
    }
}

/// An iterator over the set bit indices of a [`TrieMask`].
///
/// Iterates in ascending order by default. Use [`.rev()`](Iterator::rev) for descending order.
#[derive(Debug, Clone, Copy)]
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
        self.mask &= self.mask - 1; // Clear the lowest set bit.
        Some(bit)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = self.len();
        (count, Some(count))
    }
}

impl ExactSizeIterator for TrieMaskIter {
    #[inline]
    fn len(&self) -> usize {
        self.mask.count_ones() as usize
    }
}

impl core::iter::FusedIterator for TrieMaskIter {}

impl DoubleEndedIterator for TrieMaskIter {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.mask == 0 {
            return None;
        }
        let bit = 15 - self.mask.leading_zeros() as u8;
        self.mask &= !(1 << bit); // Clear the highest set bit.
        Some(bit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn iter_set_bits_empty() {
        let mask = TrieMask::new(0);
        assert_eq!(mask.iter().collect::<Vec<_>>(), Vec::<u8>::new());
    }

    #[test]
    fn iter_set_bits_all() {
        let mask = TrieMask::new(0xFFFF);
        assert_eq!(mask.iter().collect::<Vec<_>>(), (0..16).collect::<Vec<_>>());
    }

    #[test]
    fn iter_set_bits_sparse() {
        let mask = TrieMask::new(0b0000_0000_0010_0101); // bits 0, 2, 5
        assert_eq!(mask.iter().collect::<Vec<_>>(), vec![0, 2, 5]);
    }

    #[test]
    fn iter_set_bits_rev() {
        let mask = TrieMask::new(0b0000_0000_0010_0101); // bits 0, 2, 5
        assert_eq!(mask.iter().rev().collect::<Vec<_>>(), vec![5, 2, 0]);
    }

    #[test]
    fn iter_set_bits_double_ended() {
        let mask = TrieMask::new(0b0000_0000_0010_0101); // bits 0, 2, 5
        let mut iter = mask.iter();
        assert_eq!(iter.next(), Some(0));
        assert_eq!(iter.next_back(), Some(5));
        assert_eq!(iter.next(), Some(2));
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
    }

    #[test]
    fn iter_set_bits_exact_size() {
        let mask = TrieMask::new(0b0000_0000_0010_0101); // bits 0, 2, 5
        let mut iter = mask.iter();

        assert_eq!(iter.len(), 3);
        assert_eq!(iter.size_hint(), (3, Some(3)));

        iter.next();
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.size_hint(), (2, Some(2)));

        iter.next();
        assert_eq!(iter.len(), 1);
        assert_eq!(iter.size_hint(), (1, Some(1)));

        iter.next();
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));

        iter.next();
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
    }

    #[test]
    fn iter_set_bits_size_hint_double_ended() {
        let mask = TrieMask::new(0b0000_0000_0010_0101); // bits 0, 2, 5
        let mut iter = mask.iter();

        assert_eq!(iter.len(), 3);
        assert_eq!(iter.size_hint(), (3, Some(3)));

        iter.next();
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.size_hint(), (2, Some(2)));

        iter.next_back();
        assert_eq!(iter.len(), 1);
        assert_eq!(iter.size_hint(), (1, Some(1)));

        iter.next_back();
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
    }
}
