use core::{
    fmt,
    ops::{Bound, RangeBounds},
};

use reth_trie_common::Nibbles;
use ruint::aliases::U256;

/// This array contains 65 bitmasks used in [`PackedNibbles::slice`].
///
/// Each mask is a [`U256`] where:
/// - Index 0 is just 0 (no bits set)
/// - Index 1 has the lowest 4 bits set (one nibble)
/// - Index 2 has the lowest 8 bits set (two nibbles)
/// - ...and so on
/// - Index 64 has all bits set ([`U256::MAX`])
const SLICE_MASKS: [U256; 65] = {
    let mut masks = [U256::ZERO; 65];
    let mut i = 0;
    while i <= 64 {
        masks[i] = U256::MAX.wrapping_shr(256 - i * 4);
        i += 1;
    }
    masks
};

/// Representation for nibbles that uses a single [`U256`] to store at most 64 nibbles.
#[repr(C)] // We want to preserve the order of fields in the memory layout.
#[derive(Default, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct PackedNibbles {
    /// Nibbles length.
    // This field goes first, because the derived implementation of `PartialEq` compares the fields
    // in order, so we can short-circuit the comparison if the `length` field differs.
    pub(crate) length: u8,
    /// The nibbles themselves, stored as a 256-bit unsigned integer.
    pub(crate) nibbles: U256,
}

impl fmt::Debug for PackedNibbles {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PackedNibbles(0x{:x})", self.nibbles)
    }
}

// TODO: this can be optimized
impl From<Nibbles> for PackedNibbles {
    fn from(nibbles: Nibbles) -> Self {
        let mut packed = Self::default();
        for b in nibbles.iter().copied() {
            packed.push_unchecked(b);
        }
        packed
    }
}

// TODO: this can be optimized
impl From<PackedNibbles> for Nibbles {
    fn from(packed: PackedNibbles) -> Self {
        let mut nibbles = Self::new();
        for i in 0..packed.len() {
            nibbles.push_unchecked(packed.get_nibble(i));
        }
        nibbles
    }
}

impl PackedNibbles {
    /// Creates a new [`PackedNibbles`] instance.
    pub const fn new() -> Self {
        Self { nibbles: U256::ZERO, length: 0 }
    }

    /// Creates a new [`PackedNibbles`] instance from an iterator of nibbles.
    ///
    /// Each item in the iterator should be a nibble (0-15).
    pub fn from_nibbles(nibbles: impl IntoIterator<Item = u8>) -> Self {
        let mut packed = Self::default();
        for nibble in nibbles {
            packed.nibbles = (packed.nibbles << 4) | U256::from(nibble & 0x0F);
            packed.length += 1
        }
        packed
    }

    /// Creates a new [`PackedNibbles`] instance from an iterator of nibbles without checking
    /// bounds.
    ///
    /// Each item in the iterator should be a nibble (0-15).
    ///
    /// NOTE: This function is essentially identical to `from_nibbles`, but is kept for API
    /// compatibility.
    #[inline(always)]
    pub fn from_nibbles_unchecked(nibbles: impl IntoIterator<Item = u8>) -> Self {
        Self::from_nibbles(nibbles)
    }

    /// Creates a new [`PackedNibbles`] instance from a slice of bytes.
    pub fn unpack(bytes: impl AsRef<[u8]>) -> Self {
        Self {
            // TODO: this can be optimized
            length: (bytes.as_ref().len() * 2) as u8,
            nibbles: bytes.as_ref().iter().enumerate().fold(U256::ZERO, |mut acc, (i, byte)| {
                acc |= U256::from(*byte);
                if i < bytes.as_ref().len() {
                    acc <<= 8;
                }
                acc
            }),
        }
    }

    /// Returns the total number of bits in this [`PackedNibbles`].
    #[inline(always)]
    const fn bit_len(&self) -> usize {
        self.length as usize * 4
    }

    /// Returns `true` if this [`PackedNibbles`] is empty.
    #[inline(always)]
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Returns the total number of nibbles in this [`PackedNibbles`].
    #[inline(always)]
    pub const fn len(&self) -> usize {
        self.length as usize
    }

    /// Returns a slice of the underlying bytes.
    #[inline(always)]
    pub const fn as_slice(&self) -> &[u8] {
        self.nibbles.as_le_slice()
    }

    /// Gets the nibble at the given position.
    ///
    /// # Panics
    ///
    /// If the position is out of bounds.
    pub const fn get_nibble(&self, pos: usize) -> u8 {
        debug_assert!(pos < self.len());

        // How far from the most-significant nibble?
        let pos_from_back = self.len() - 1 - pos; // 0-based from MSB
        let limb = pos_from_back / 16; // 16 nibbles per u64 limb
        let offset = (pos_from_back & 0xF) * 4; // Offset bits within that limb, so we get the one we're interested in

        let word = self.nibbles.as_limbs()[limb];
        ((word >> offset) & 0xF) as u8
    }

    /// Returns the last nibble in this [`PackedNibbles`], or `None` if empty.
    pub const fn last(&self) -> Option<u8> {
        if self.is_empty() {
            return None;
        }

        Some(self.get_nibble(self.len() - 1))
    }

    /// Returns `true` if this [`PackedNibbles`] starts with the nibbles in `other`.
    pub const fn starts_with(&self, other: &Self) -> bool {
        // If other is empty, it's a prefix of any sequence
        if other.is_empty() {
            return true;
        }

        // If other is longer than self, it can't be a prefix
        if other.len() > self.len() {
            return false;
        }

        let mut i = 0;
        while i < other.len() {
            if self.get_nibble(i) != other.get_nibble(i) {
                return false;
            }
            i += 1;
        }

        true
    }

    /// Returns the length of the common prefix between this [`PackedNibbles`] and `other`.
    pub const fn common_prefix_length(&self, other: &Self) -> usize {
        const fn count_equal_nibbles(self_limb: u64, other_limb: u64) -> usize {
            // Pad both limbs with trailing zeros to the same effective length
            let lhs_bit_len = u64::BITS - self_limb.leading_zeros(); // Effective bit length of the left limb
            let rhs_bit_len = u64::BITS - other_limb.leading_zeros(); // Effective bit length of the right limb
            let diff = lhs_bit_len as isize - rhs_bit_len as isize; // Difference in bit lengths
            let (lhs, rhs) = if diff < 0 {
                (self_limb << -diff, other_limb)
            } else {
                (self_limb, other_limb << diff)
            }; // Pad one of the limbs

            // Count equal leading bits
            let lz_or = (lhs | rhs).leading_zeros(); // Leading zeros common to both limbs
            let skip = lz_or & !0b11u32; // Leading zeros common to both limbs, rounded down to the nearest nibble
            let lz_xor = (lhs ^ rhs).leading_zeros(); // Leading bits common to both limbs
            (lz_xor - skip) as usize / 4
        }

        let self_bit_len = self.bit_len();
        let other_bit_len = other.bit_len();

        if self_bit_len == 0 || other_bit_len == 0 {
            return 0
        }

        let min_bit_len = if self_bit_len < other_bit_len { self_bit_len } else { other_bit_len };

        // Number of whole limbs
        let full_limbs = min_bit_len / 64;

        let self_limbs = self.nibbles.as_limbs();
        let other_limbs = other.nibbles.as_limbs();
        let mut common_nibbles = 0;

        // Walk from MS-limb to LS-limb
        let mut i = full_limbs;
        while i > 0 {
            i -= 1;
            if self_limbs[i] == other_limbs[i] {
                common_nibbles += 16;
            } else {
                // First differing limb – count equal nibbles inside it
                common_nibbles += count_equal_nibbles(self_limbs[i], other_limbs[i]);
                return common_nibbles;
            }
        }

        if min_bit_len % 64 == 0 {
            return common_nibbles;
        }

        common_nibbles + count_equal_nibbles(self_limbs[0], other_limbs[0])
    }

    /// Creates a new [`PackedNibbles`] containing the nibbles in the specified range `[start, end)`
    /// without checking bounds.
    ///
    /// # Safety
    ///
    /// This method does not verify that the provided range is valid for this [`PackedNibbles`].
    /// The caller must ensure that `start <= end` and `end <= self.len()`.
    pub const fn slice_unchecked(&self, start: usize, end: usize) -> Self {
        // Fast path for empty slice
        if start == end {
            return Self::new();
        }

        // Fast path for full slice
        let slice_to_end = end == self.len();
        if start == 0 && slice_to_end {
            return *self;
        }

        let nibble_len = end - start;

        // Shift so that the first requested nibble becomes the least significant one
        let shifted = if slice_to_end {
            self.nibbles
        } else {
            let shift = (self.len() - end) * 4;
            self.nibbles.wrapping_shr(shift)
        };
        // Mask out everything to the left of the slice
        let nibbles = shifted.bitand(SLICE_MASKS[nibble_len]);

        Self { length: nibble_len as u8, nibbles }
    }

    /// Creates a new [`PackedNibbles`] containing the nibbles in the specified range.
    ///
    /// # Panics
    ///
    /// This method will panic if the range is out of bounds for this [`PackedNibbles`].
    pub fn slice(&self, range: impl RangeBounds<usize>) -> Self {
        // Determine the start and end nibble indices from the range bounds
        let start = match range.start_bound() {
            Bound::Included(&idx) => idx,
            Bound::Excluded(&idx) => idx + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&idx) => idx + 1,
            Bound::Excluded(&idx) => idx,
            Bound::Unbounded => self.len(),
        };
        assert!(start <= end, "Cannot slice with a start index greater than the end index");
        assert!(
            end <= self.len(),
            "Cannot slice with an end index greater than the length of the nibbles"
        );

        self.slice_unchecked(start, end)
    }

    /// Pushes a single nibble to the end of the nibbles.
    ///
    /// This will only look at the low nibble of the byte passed, i.e. for `0x02` the nibble `2`
    /// will be pushed.
    ///
    /// NOTE: if there is data in the high nibble, it will be ignored.
    pub const fn push_unchecked(&mut self, nibble: u8) {
        if self.length > 0 {
            self.nibbles = self.nibbles.wrapping_shl(4);
        }
        self.nibbles = self.nibbles.bitor(U256::from_limbs([(nibble & 0x0F) as u64, 0, 0, 0]));
        self.length += 1;
    }

    /// Extends this [`PackedNibbles`] with the given [`PackedNibbles`].
    pub const fn extend_path(&mut self, other: &Self) {
        if other.is_empty() {
            return;
        }

        self.nibbles = self.nibbles.wrapping_shl(other.bit_len()).bitor(other.nibbles);
        self.length += other.length;
    }

    /// Truncates this [`PackedNibbles`] to the specified length, in nibbles.
    pub const fn truncate(&mut self, new_len: usize) {
        assert!(
            new_len <= self.len(),
            "Cannot truncate to a length greater than the current length"
        );
        *self = self.slice_unchecked(0, new_len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packed_nibbles_from_nibbles() {
        let a = PackedNibbles::from_nibbles([1, 2, 3]);
        assert_eq!(format!("{a:?}"), "PackedNibbles(0x123)")
    }

    #[test]
    fn test_packed_nibbles_clone() {
        let a = PackedNibbles::from_nibbles([1, 2, 3]);
        #[allow(clippy::redundant_clone)]
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn test_packed_nibbles_ord() {
        // Test empty nibbles
        let nibbles1 = PackedNibbles::default();
        let nibbles2 = PackedNibbles::default();
        assert_eq!(nibbles1.cmp(&nibbles2), core::cmp::Ordering::Equal);

        // Test with same nibbles
        let nibbles1 = PackedNibbles::unpack([0x12, 0x34]);
        let nibbles2 = PackedNibbles::unpack([0x12, 0x34]);
        assert_eq!(nibbles1.cmp(&nibbles2), core::cmp::Ordering::Equal);

        // Test with different lengths
        let short = PackedNibbles::unpack([0x12]);
        let long = PackedNibbles::unpack([0x12, 0x34]);
        assert_eq!(short.cmp(&long), core::cmp::Ordering::Less);

        // Test with common prefix but different values
        let nibbles1 = PackedNibbles::unpack([0x12, 0x34]);
        let nibbles2 = PackedNibbles::unpack([0x12, 0x35]);
        assert_eq!(nibbles1.cmp(&nibbles2), core::cmp::Ordering::Less);

        // Test with differing first byte
        let nibbles1 = PackedNibbles::unpack([0x12, 0x34]);
        let nibbles2 = PackedNibbles::unpack([0x13, 0x34]);
        assert_eq!(nibbles1.cmp(&nibbles2), core::cmp::Ordering::Less);

        // Test with odd length nibbles
        let nibbles1 = PackedNibbles::unpack([0x1]);
        let nibbles2 = PackedNibbles::unpack([0x2]);
        assert_eq!(nibbles1.cmp(&nibbles2), core::cmp::Ordering::Less);

        // Test with odd and even length nibbles
        let odd = PackedNibbles::unpack([0x1]);
        let even = PackedNibbles::unpack([0x12]);
        assert_eq!(odd.cmp(&even), core::cmp::Ordering::Less);

        // Test with longer sequences
        let nibbles1 = PackedNibbles::unpack([0x12, 0x34, 0x56, 0x78]);
        let nibbles2 = PackedNibbles::unpack([0x12, 0x34, 0x56, 0x79]);
        assert_eq!(nibbles1.cmp(&nibbles2), core::cmp::Ordering::Less);
    }

    #[test]
    fn test_packed_nibbles_starts_with() {
        let nibbles = PackedNibbles::from_nibbles([1, 2, 3, 4]);

        // Test empty nibbles
        let empty = PackedNibbles::default();
        assert!(nibbles.starts_with(&empty));
        assert!(empty.starts_with(&empty));
        assert!(!empty.starts_with(&nibbles));

        // Test with same nibbles
        assert!(nibbles.starts_with(&nibbles));

        // Test with prefix
        let prefix = PackedNibbles::from_nibbles([1, 2]);
        assert!(nibbles.starts_with(&prefix));
        assert!(!prefix.starts_with(&nibbles));

        // Test with different first nibble
        let different = PackedNibbles::from_nibbles([2, 2, 3, 4]);
        assert!(!nibbles.starts_with(&different));

        // Test with longer sequence
        let longer = PackedNibbles::from_nibbles([1, 2, 3, 4, 5, 6]);
        assert!(!nibbles.starts_with(&longer));

        // Test with even nibbles and odd prefix
        let even_nibbles = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        let odd_prefix = PackedNibbles::from_nibbles([1, 2, 3]);
        assert!(even_nibbles.starts_with(&odd_prefix));

        // Test with odd nibbles and even prefix
        let odd_nibbles = PackedNibbles::from_nibbles([1, 2, 3]);
        let even_prefix = PackedNibbles::from_nibbles([1, 2]);
        assert!(odd_nibbles.starts_with(&even_prefix));
    }

    #[test]
    fn test_packed_nibbles_slice() {
        // Test with empty nibbles
        let empty = PackedNibbles::default();
        assert_eq!(empty.slice(..), empty);

        // Test with even number of nibbles
        let even = PackedNibbles::from_nibbles([0, 1, 2, 3, 4, 5]);

        // Full slice
        assert_eq!(even.slice(..), even);
        assert_eq!(even.slice(0..6), even);

        // Empty slice
        assert_eq!(even.slice(3..3), PackedNibbles::default());

        // Beginning slices (even start)
        assert_eq!(even.slice(0..2), PackedNibbles::from_nibbles(0..2));

        // Middle slices (even start, even end)
        assert_eq!(even.slice(2..4), PackedNibbles::from_nibbles(2..4));

        // End slices (even start)
        assert_eq!(even.slice(4..6), PackedNibbles::from_nibbles(4..6));

        // Test with odd number of nibbles
        let odd = PackedNibbles::from_nibbles(0..5);

        // Full slice
        assert_eq!(odd.slice(..), odd);
        assert_eq!(odd.slice(0..5), odd);

        // Beginning slices (odd length)
        assert_eq!(odd.slice(0..3), PackedNibbles::from_nibbles(0..3));

        // Middle slices with odd start
        assert_eq!(odd.slice(1..4), PackedNibbles::from_nibbles(1..4));

        // Middle slices with odd end
        assert_eq!(odd.slice(1..3), PackedNibbles::from_nibbles(1..3));

        // End slices (odd start)
        assert_eq!(odd.slice(2..5), PackedNibbles::from_nibbles(2..5));

        // Special cases - both odd start and end
        assert_eq!(odd.slice(1..4), PackedNibbles::from_nibbles(1..4));

        // Single nibble slices
        assert_eq!(even.slice(2..3), PackedNibbles::from_nibbles(2..3));

        assert_eq!(even.slice(3..4), PackedNibbles::from_nibbles(3..4));

        // Test with alternate syntax
        assert_eq!(even.slice(2..), PackedNibbles::from_nibbles(2..6));
        assert_eq!(even.slice(..4), PackedNibbles::from_nibbles(0..4));
        assert_eq!(even.slice(..=3), PackedNibbles::from_nibbles(0..4));

        // More complex test case with the max length array sliced at the end
        assert_eq!(
            PackedNibbles::from_nibbles(0..64).slice(1..),
            PackedNibbles::from_nibbles(1..64)
        );
    }

    #[test]
    fn test_packed_nibbles_common_prefix_length() {
        // Test with empty nibbles
        let empty = PackedNibbles::default();
        assert_eq!(empty.common_prefix_length(&empty), 0);

        // Test with same nibbles
        let nibbles1 = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        let nibbles2 = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        assert_eq!(nibbles1.common_prefix_length(&nibbles2), 4);
        assert_eq!(nibbles2.common_prefix_length(&nibbles1), 4);

        // Test with partial common prefix (byte aligned)
        let nibbles1 = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        let nibbles2 = PackedNibbles::from_nibbles([1, 2, 5, 6]);
        assert_eq!(nibbles1.common_prefix_length(&nibbles2), 2);
        assert_eq!(nibbles2.common_prefix_length(&nibbles1), 2);

        // Test with partial common prefix (half-byte aligned)
        let nibbles1 = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        let nibbles2 = PackedNibbles::from_nibbles([1, 2, 3, 7]);
        assert_eq!(nibbles1.common_prefix_length(&nibbles2), 3);
        assert_eq!(nibbles2.common_prefix_length(&nibbles1), 3);

        // Test with no common prefix
        let nibbles1 = PackedNibbles::from_nibbles([5, 6, 7, 8]);
        let nibbles2 = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        assert_eq!(nibbles1.common_prefix_length(&nibbles2), 0);
        assert_eq!(nibbles2.common_prefix_length(&nibbles1), 0);

        // Test with different lengths but common prefix
        let nibbles1 = PackedNibbles::from_nibbles([1, 2, 3, 4, 5, 6]);
        let nibbles2 = PackedNibbles::from_nibbles([1, 2, 3]);
        assert_eq!(nibbles1.common_prefix_length(&nibbles2), 3);
        assert_eq!(nibbles2.common_prefix_length(&nibbles1), 3);

        // Test with odd number of nibbles
        let nibbles1 = PackedNibbles::from_nibbles([1, 2, 3]);
        let nibbles2 = PackedNibbles::from_nibbles([1, 2, 7]);
        assert_eq!(nibbles1.common_prefix_length(&nibbles2), 2);
        assert_eq!(nibbles2.common_prefix_length(&nibbles1), 2);

        // Test with half-byte difference in first byte
        let nibbles1 = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        let nibbles2 = PackedNibbles::from_nibbles([5, 2, 3, 4]);
        assert_eq!(nibbles1.common_prefix_length(&nibbles2), 0);
        assert_eq!(nibbles2.common_prefix_length(&nibbles1), 0);

        // Test with one empty and one non-empty
        let nibbles1 = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        assert_eq!(nibbles1.common_prefix_length(&empty), 0);
        assert_eq!(empty.common_prefix_length(&nibbles1), 0);

        // Test with longer sequences (16 nibbles)
        let nibbles1 =
            PackedNibbles::from_nibbles([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0]);
        let nibbles2 =
            PackedNibbles::from_nibbles([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 1]);
        assert_eq!(nibbles1.common_prefix_length(&nibbles2), 15);
        assert_eq!(nibbles2.common_prefix_length(&nibbles1), 15);

        // Test with different lengths but same prefix (32 vs 16 nibbles)
        let nibbles1 =
            PackedNibbles::from_nibbles([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0]);
        let nibbles2 = PackedNibbles::from_nibbles([
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10,
            11, 12, 13, 14, 15, 0,
        ]);
        assert_eq!(nibbles1.common_prefix_length(&nibbles2), 16);
        assert_eq!(nibbles2.common_prefix_length(&nibbles1), 16);

        // Test with very long sequences (32 nibbles) with different endings
        let nibbles1 = PackedNibbles::from_nibbles([
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10,
            11, 12, 13, 14, 15, 0,
        ]);
        let nibbles2 = PackedNibbles::from_nibbles([
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10,
            11, 12, 13, 14, 15, 1,
        ]);
        assert_eq!(nibbles1.common_prefix_length(&nibbles2), 31);
        assert_eq!(nibbles2.common_prefix_length(&nibbles1), 31);
    }

    #[test]
    fn test_packed_nibbles_truncate() {
        // Test truncating empty nibbles
        let mut nibbles = PackedNibbles::default();
        nibbles.truncate(0);
        assert_eq!(nibbles, PackedNibbles::default());

        // Test truncating to zero length
        let mut nibbles = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        nibbles.truncate(0);
        assert_eq!(nibbles, PackedNibbles::default());

        // Test truncating to same length (should be no-op)
        let mut nibbles = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        nibbles.truncate(4);
        assert_eq!(nibbles, PackedNibbles::from_nibbles([1, 2, 3, 4]));

        // Individual nibble test with a simple 2-nibble truncation
        let mut nibbles = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        nibbles.truncate(2);
        assert_eq!(nibbles, PackedNibbles::from_nibbles([1, 2]));

        // Test simple truncation
        let mut nibbles = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        nibbles.truncate(2);
        assert_eq!(nibbles, PackedNibbles::from_nibbles([1, 2]));

        // Test truncating to single nibble
        let mut nibbles = PackedNibbles::from_nibbles([5, 6, 7, 8]);
        nibbles.truncate(1);
        assert_eq!(nibbles, PackedNibbles::from_nibbles([5]));
    }
}
