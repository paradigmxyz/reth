use alloy_trie::TrieMask;

/// A dense index into a branch node's blinded children array.
///
/// Branch nodes store blinded children densely — only blinded nibble slots have entries.
/// This type wraps the `u8` index into that dense array, providing safe construction
/// from a `(TrieMask, nibble)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BranchChildIdx(u8);

impl BranchChildIdx {
    /// Returns the dense index for `nibble` within the children array described by `mask`.
    ///
    /// Returns `None` if the nibble's bit is not set in `mask`.
    pub(super) const fn new(mask: TrieMask, nibble: u8) -> Option<Self> {
        if !mask.is_bit_set(nibble) {
            return None;
        }
        Some(Self(Self::count_below(mask, nibble)))
    }

    /// Returns the dense index as a `usize`, suitable for indexing into a `Vec` or slice.
    pub(super) const fn get(self) -> usize {
        self.0 as usize
    }

    /// Counts the number of occupied child slots below `nibble` in the dense children array.
    const fn count_below(mask: TrieMask, nibble: u8) -> u8 {
        (mask.get() & ((1u16 << nibble) - 1)).count_ones() as u8
    }
}
