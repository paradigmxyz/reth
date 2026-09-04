use alloy_trie::{TrieMask, TrieMaskIter};
use core::ops::{Index, IndexMut};
use smallvec::SmallVec;

/// A dense index into a branch node's children array.
///
/// Branch nodes store children densely — only occupied nibble slots have entries. This type
/// wraps the `u8` index into that dense array, providing safe construction from a
/// `(TrieMask, nibble)` pair and ergonomic indexing into `SmallVec` or slices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BranchChildIdx(u8);

impl BranchChildIdx {
    /// Returns the dense index for `nibble` within the children array of a branch whose
    /// occupied slots are described by `state_mask`.
    ///
    /// Returns `None` if the nibble's bit is not set in `state_mask`.
    pub(super) const fn new(state_mask: TrieMask, nibble: u8) -> Option<Self> {
        if !state_mask.is_bit_set(nibble) {
            return None;
        }
        Some(Self::new_unchecked(state_mask, nibble))
    }

    /// Returns the dense insertion point for `nibble` — the number of occupied child slots
    /// below `nibble`. Unlike [`Self::new`], this does **not** require the nibble's bit to be
    /// set, making it suitable for computing the position at which a new child should be
    /// inserted.
    pub(super) const fn insertion_point(state_mask: TrieMask, nibble: u8) -> Self {
        Self(Self::count_below(state_mask, nibble))
    }

    /// Returns the dense index as a `usize`, suitable for indexing into a `SmallVec` or slice.
    pub(super) const fn get(self) -> usize {
        self.0 as usize
    }

    /// Counts the number of occupied child slots below `nibble` in the dense children array.
    const fn count_below(state_mask: TrieMask, nibble: u8) -> u8 {
        (state_mask.get() & ((1u16 << nibble) - 1)).count_ones() as u8
    }

    /// Computes the dense index for `nibble` without checking whether the bit is set.
    const fn new_unchecked(state_mask: TrieMask, nibble: u8) -> Self {
        Self(Self::count_below(state_mask, nibble))
    }
}

impl<T> Index<BranchChildIdx> for SmallVec<[T; 4]> {
    type Output = T;

    fn index(&self, idx: BranchChildIdx) -> &Self::Output {
        &self.as_slice()[idx.get()]
    }
}

impl<T> IndexMut<BranchChildIdx> for SmallVec<[T; 4]> {
    fn index_mut(&mut self, idx: BranchChildIdx) -> &mut Self::Output {
        &mut self.as_mut_slice()[idx.get()]
    }
}

/// An iterator over a branch's children that yields `(BranchChildIdx, nibble)` pairs.
///
/// Tracks the dense index separately so traversal can resume at an arbitrary nibble.
pub(super) struct BranchChildIter {
    inner: TrieMaskIter,
    dense: u8,
}

impl BranchChildIter {
    /// Creates a new iterator over the occupied children of the given `state_mask`.
    pub(super) const fn new(state_mask: TrieMask) -> Self {
        Self { inner: state_mask.iter(), dense: 0 }
    }

    /// Resumes iteration at `nibble`, preserving indices in the full dense children array.
    /// A nibble of 16 produces an empty iterator.
    pub(super) const fn from_nibble(state_mask: TrieMask, nibble: u8) -> Self {
        debug_assert!(nibble <= 16);
        let lower_bits = ((1u32 << nibble) - 1) as u16;
        Self {
            inner: TrieMask::new(state_mask.get() & !lower_bits).iter(),
            dense: (state_mask.get() & lower_bits).count_ones() as u8,
        }
    }
}

impl Iterator for BranchChildIter {
    type Item = (BranchChildIdx, u8);

    fn next(&mut self) -> Option<Self::Item> {
        let nibble = self.inner.next()?;
        let index = BranchChildIdx(self.dense);
        self.dense += 1;
        Some((index, nibble))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_at_every_nibble_for_every_occupancy_mask() {
        for mask in 0..=u16::MAX {
            for start in 0..=16 {
                let mut children = BranchChildIter::from_nibble(TrieMask::new(mask), start);
                let mut dense = 0;
                for nibble in 0..16 {
                    if mask & (1 << nibble) != 0 {
                        if nibble >= start {
                            assert_eq!(children.next(), Some((BranchChildIdx(dense), nibble)));
                        }
                        dense += 1;
                    }
                }
                assert_eq!(children.next(), None);
            }
        }
    }
}
