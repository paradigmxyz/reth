use core::{
    fmt,
    ops::{Bound, RangeBounds},
};

use alloy_primitives::U256;
use reth_trie_common::Nibbles;

/// A representation for nibbles, that uses an even/odd flag.
#[repr(C)] // We when to preserve the order of fields in the memory layout
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
    /// Creates a new `PackedNibbles` instance from an iterator of nibbles.
    ///
    /// Each item in the iterator should be a nibble (0-15).
    pub fn from_nibbles(nibbles: impl IntoIterator<Item = u8>) -> Self {
        let mut nibbles = nibbles.into_iter().peekable();
        let mut packed = Self::default();
        while let Some(nibble) = nibbles.next() {
            packed.nibbles |= U256::from(nibble);
            if nibbles.peek().is_some() {
                packed.nibbles <<= 4;
            }
            packed.length += 1
        }
        packed
    }

    /// Creates a new `PackedNibbles` instance from an iterator of nibbles without checking bounds.
    ///
    /// Each item in the iterator should be a nibble (0-15).
    /// This function is essentially identical to `from_nibbles` but is kept for API compatibility.
    pub fn from_nibbles_unchecked(nibbles: impl IntoIterator<Item = u8>) -> Self {
        Self::from_nibbles(nibbles)
    }

    /// Creates a new `PackedNibbles` instance from a slice of bytes.
    ///
    /// This treats each byte as a single element rather than unpacking into nibbles.
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

    const fn bit_len(&self) -> usize {
        self.length as usize * 4
    }

    /// Returns `true` if this [`PackedNibbles`] is empty.
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Returns `true` if this [`PackedNibbles`] starts with the nibbles in `other`.
    pub fn starts_with(&self, other: &Self) -> bool {
        // If other is empty, it's a prefix of any sequence
        if other.is_empty() {
            return true;
        }

        // If other is longer than self, it can't be a prefix
        if other.len() > self.len() {
            return false;
        }

        (self.nibbles >> ((self.bit_len()).saturating_sub(other.bit_len()))) & other.nibbles ==
            other.nibbles
    }

    /// Returns the total number of nibbles in this [`PackedNibbles`].
    pub const fn len(&self) -> usize {
        self.length as usize
    }

    /// Returns a slice of the underlying bytes.
    pub const fn as_slice(&self) -> &[u8] {
        self.nibbles.as_le_slice()
    }

    /// Returns the length of the common prefix between this [`PackedNibbles`] and `other`.
    pub fn common_prefix_length(&self, other: &Self) -> usize {
        // Calculate the max bit length of two U256s
        let self_bit_len = self.bit_len();
        let other_bit_len = other.bit_len();
        let max_bit_len = self_bit_len.max(other_bit_len);

        // Get both U256s to the same bit length, and then XOR them
        let self_adjusted = self.nibbles << (max_bit_len - self_bit_len);
        let other_adjusted = other.nibbles << (max_bit_len - other_bit_len);
        let xor = self_adjusted ^ other_adjusted;

        // Calculate the number of leading zeros, excluding those that were already present
        let zeros = xor.leading_zeros() - (U256::BITS - max_bit_len);

        //
        zeros.min(self_bit_len.min(other_bit_len)) / 4
    }

    /// Returns the last nibble in this [`PackedNibbles`], or `None` if empty.
    pub const fn last(&self) -> Option<u8> {
        if self.is_empty() {
            return None;
        }

        Some(self.get_nibble(self.len() - 1))
    }

    /// Gets the nibble at the given position.
    /// # Panics
    ///
    /// Panics if the position is out of bounds.
    pub const fn get_nibble(&self, pos: usize) -> u8 {
        let byte_pos = pos / 2;
        let byte = self.nibbles.byte(byte_pos);

        if pos % 2 == 0 {
            // For even positions, return the high nibble
            (byte & 0xF0) >> 4
        } else {
            // For odd positions, return the low nibble
            byte & 0x0F
        }
    }

    /// Creates a new [`PackedNibbles`] containing the nibbles in the specified range.
    ///
    /// # Panics
    ///
    /// This method will panic if the range is out of bounds for this [`PackedNibbles`].
    pub fn slice(&self, range: impl RangeBounds<usize>) -> Self {
        // Determine the start and end nibble indices from the range bounds
        let len = self.len();
        let start = match range.start_bound() {
            Bound::Included(&idx) => idx,
            Bound::Excluded(&idx) => idx + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&idx) => idx + 1,
            Bound::Excluded(&idx) => idx,
            Bound::Unbounded => len,
        };
        assert!(start <= end && end <= len);

        // Fast path for empty slice
        if start == end {
            return Self::default();
        }

        // Fast path for full slice
        if start == 0 && end == len {
            return *self;
        }

        let nibble_count = end - start;

        let from_byte = start / 2;
        let to_byte = end.div_ceil(2);

        let byte_count = to_byte - from_byte;

        let bytes: [u8; 32] = self.nibbles.to_be_bytes();

        let mut out = Vec::with_capacity(byte_count);
        unsafe {
            // SAFETY: `out` is a valid contiguous slice of length
            // `CAPACITY_BYTES` non-overlapping with `self.nibbles`.
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr().add(from_byte),
                out.as_mut_ptr(),
                byte_count,
            );
        }

        // Patch first and/or last byte, if needed.
        let start_odd = start % 2 != 0;
        let end_odd = end % 2 != 0;

        // If we start on an odd nibble, we need to shift everything left by 4 bits.
        //
        // For `out = [0x12, 0x34]`, it will be turned into `[0x23, 0x40]`.
        if start_odd {
            if byte_count > 1 {
                for i in 0..byte_count - 1 {
                    // SAFETY: all accesses are within bounds, because `out` is initialized with
                    // exactly `byte_count` bytes.
                    unsafe {
                        let next = *out.get_unchecked(i + 1);
                        let current = out.get_unchecked_mut(i);
                        *current = (*current << 4) | (next >> 4);
                    }
                }
            }
            // SAFETY: access is within bounds, because `out` is initialized with exactly
            // `byte_count` bytes.
            let last = unsafe { out.get_unchecked_mut(byte_count - 1) };
            *last <<= 4;
        }

        if end_odd {
            // SAFETY: access is within bounds, because `out` is initialized with exactly
            // `byte_count` bytes.
            let last = unsafe { out.get_unchecked_mut(byte_count - 1) };
            *last &= 0xF0;
        }

        // SAFETY: We've already checked that the length is valid, and we're setting the length
        // to a smaller value.
        unsafe { out.set_len(nibble_count.div_ceil(2)) };

        Self { length: nibble_count as u8, nibbles: U256::from_be_bytes(bytes) }
    }

    /// Extends this [`PackedNibbles`] with the given [`PackedNibbles`].
    pub fn extend_path(&mut self, other: &Self) {
        if other.is_empty() {
            // If `other` is empty, we can just return
            return;
        }

        self.nibbles <<= other.bit_len();
        self.nibbles |= other.nibbles;
        self.length += other.length;
    }

    /// Pushes a single nibble to the end of the nibbles.
    ///
    /// This will only look at the low nibble of the byte passed, i.e. for `0x02` the nibble `2`
    /// will be pushed.
    ///
    /// NOTE: if there is data in the high nibble, it will be ignored.
    pub fn push_unchecked(&mut self, nibble: u8) {
        if self.length > 0 {
            self.nibbles <<= 4;
        }
        self.nibbles |= U256::from(nibble & 0x0F);
        self.length += 1;
    }

    /// Truncates this [`PackedNibbles`] to the specified length.
    pub fn truncate(&mut self, new_len: usize) {
        self.length = new_len as u8;
        self.nibbles &= (U256::from(1) << new_len) - U256::from(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packed_nibles_from_nibbles() {
        let a = PackedNibbles::from_nibbles([1, 2, 3]);
        println!("{:0width$b}", a.nibbles, width = a.bit_len());
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
        let empty1 = PackedNibbles::default();
        let empty2 = PackedNibbles::default();
        assert_eq!(empty1.cmp(&empty2), core::cmp::Ordering::Equal);

        // Test with same nibbles
        let a = PackedNibbles::unpack([0x12, 0x34]);
        let b = PackedNibbles::unpack([0x12, 0x34]);
        assert_eq!(a.cmp(&b), core::cmp::Ordering::Equal);

        // Test with different lengths
        let short = PackedNibbles::unpack([0x12]);
        let long = PackedNibbles::unpack([0x12, 0x34]);
        assert_eq!(short.cmp(&long), core::cmp::Ordering::Less);

        // Test with common prefix but different values
        let c = PackedNibbles::unpack([0x12, 0x34]);
        let d = PackedNibbles::unpack([0x12, 0x35]);
        assert_eq!(c.cmp(&d), core::cmp::Ordering::Less);

        // Test with differing first byte
        let e = PackedNibbles::unpack([0x12, 0x34]);
        let f = PackedNibbles::unpack([0x13, 0x34]);
        assert_eq!(e.cmp(&f), core::cmp::Ordering::Less);

        // Test with odd length nibbles
        let odd1 = PackedNibbles::unpack([0x1]);
        let odd2 = PackedNibbles::unpack([0x2]);
        assert_eq!(odd1.cmp(&odd2), core::cmp::Ordering::Less);

        // Test with odd and even length nibbles
        let odd = PackedNibbles::unpack([0x1]);
        let even = PackedNibbles::unpack([0x12]);
        assert_eq!(odd.cmp(&even), core::cmp::Ordering::Less);

        // Test with longer sequences
        let long1 = PackedNibbles::unpack([0x12, 0x34, 0x56, 0x78]);
        let long2 = PackedNibbles::unpack([0x12, 0x34, 0x56, 0x79]);
        assert_eq!(long1.cmp(&long2), core::cmp::Ordering::Less);
    }

    #[test]
    fn test_packed_nibbles_starts_with() {
        let a = PackedNibbles::from_nibbles([1, 2, 3, 4]);

        // Test empty nibbles
        let empty = PackedNibbles::default();
        assert!(a.starts_with(&empty));
        assert!(empty.starts_with(&empty));
        assert!(!empty.starts_with(&a));

        // Test with same nibbles
        assert!(a.starts_with(&a));

        // Test with prefix
        let prefix = PackedNibbles::from_nibbles([1, 2]);
        assert!(a.starts_with(&prefix));
        assert!(!prefix.starts_with(&a));

        // Test with different first nibble
        let different = PackedNibbles::from_nibbles([2, 2, 3, 4]);
        assert!(!a.starts_with(&different));

        // Test with longer sequence
        let longer = PackedNibbles::from_nibbles([1, 2, 3, 4, 5, 6]);
        assert!(!a.starts_with(&longer));

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
        let a = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        let b = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        assert_eq!(a.common_prefix_length(&b), 4);
        assert_eq!(b.common_prefix_length(&a), 4);

        // Test with partial common prefix (byte aligned)
        let c = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        let d = PackedNibbles::from_nibbles([1, 2, 5, 6]);
        assert_eq!(c.common_prefix_length(&d), 2);
        assert_eq!(d.common_prefix_length(&c), 2);

        // Test with partial common prefix (half-byte aligned)
        let e = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        let f = PackedNibbles::from_nibbles([1, 2, 3, 7]);
        assert_eq!(e.common_prefix_length(&f), 3);
        assert_eq!(f.common_prefix_length(&e), 3);

        // Test with no common prefix
        let g = PackedNibbles::from_nibbles([5, 6, 7, 8]);
        let h = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        assert_eq!(g.common_prefix_length(&h), 0);
        assert_eq!(h.common_prefix_length(&g), 0);

        // Test with different lengths but common prefix
        let i = PackedNibbles::from_nibbles([1, 2, 3, 4, 5, 6]);
        let j = PackedNibbles::from_nibbles([1, 2, 3]);
        assert_eq!(i.common_prefix_length(&j), 3);
        assert_eq!(j.common_prefix_length(&i), 3);

        // Test with odd number of nibbles
        let k = PackedNibbles::from_nibbles([1, 2, 3]);
        let l = PackedNibbles::from_nibbles([1, 2, 7]);
        assert_eq!(k.common_prefix_length(&l), 2);
        assert_eq!(l.common_prefix_length(&k), 2);

        // Test with half-byte difference in first byte
        let m = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        let n = PackedNibbles::from_nibbles([5, 2, 3, 4]);
        assert_eq!(m.common_prefix_length(&n), 0);
        assert_eq!(n.common_prefix_length(&m), 0);

        // Test with one empty and one non-empty
        let o = PackedNibbles::from_nibbles([1, 2, 3, 4]);
        assert_eq!(o.common_prefix_length(&empty), 0);
        assert_eq!(empty.common_prefix_length(&o), 0);
    }
}
