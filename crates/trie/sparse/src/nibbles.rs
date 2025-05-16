use core::{
    fmt,
    ops::{Bound, RangeBounds},
};

use reth_trie_common::Nibbles;
use tinyvec::ArrayVec;

/// The capacity of the underlying byte array. This limits the maximum size of [`PackedNibbles`] to
/// 64 nibbles, or 256 bits.
const CAPACITY_BYTES: usize = 32;

/// A representation for nibbles, that uses an even/odd flag.
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct PackedNibbles {
    /// The even/odd flag, indicating whether the length is even or odd.
    // This field goes first, because the derived implementation of `PartialEq` compares the fields
    // in order, so we can short-circuit the comparison if the `even` flag is different.
    pub(crate) even: bool,
    /// The nibbles themselves, stored as a byte array.
    pub(crate) nibbles: ArrayVec<[u8; CAPACITY_BYTES]>,
}

impl Default for PackedNibbles {
    fn default() -> Self {
        Self { even: true, nibbles: ArrayVec::new() }
    }
}

impl fmt::Debug for PackedNibbles {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PackedNibbles(0x")?;

        // For all bytes except potentially the last one, print the full byte
        let full_bytes = if self.even { self.nibbles.len() } else { self.nibbles.len() - 1 };

        for &byte in &self.nibbles[0..full_bytes] {
            write!(f, "{:02x}", byte)?;
        }

        // If odd, print only the high nibble of the last byte
        if !self.even && !self.nibbles.is_empty() {
            let last_byte = self.nibbles[self.nibbles.len() - 1];
            write!(f, "{:x}", (last_byte & 0xF0) >> 4)?;
        }

        write!(f, ")")
    }
}

impl PartialOrd for PackedNibbles {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// TODO: this can be optimized
impl Ord for PackedNibbles {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        // Get the lengths of both byte sequences
        let self_len = self.nibbles.len();
        let other_len = other.nibbles.len();

        // Compare bytes up to the common length
        let min_len = core::cmp::min(self_len, other_len);
        let self_common = &self.nibbles[..min_len];
        let other_common = &other.nibbles[..min_len];
        if self_common != other_common {
            return self_common.cmp(other_common);
        }

        // If we've compared all bytes and they're equal up to the min length,
        // compare the lengths
        match self_len.cmp(&other_len) {
            core::cmp::Ordering::Equal => {
                // Lengths are equal, compare the parity
                match (self.even, other.even) {
                    (true, true) | (false, false) => core::cmp::Ordering::Equal,
                    // For example, the same byte 0x60 will be represented as "60" for
                    // `self` and "6" for `other`
                    (true, false) => core::cmp::Ordering::Greater,
                    // For example, the same byte 0x60 will be represented as "6" for
                    // `self` and "60" for `other`
                    (false, true) => core::cmp::Ordering::Less,
                }
            }
            len_cmp => {
                // Lengths are different, so just order by length
                len_cmp
            }
        }
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
    pub const fn new() -> Self {
        Self { even: true, nibbles: ArrayVec::from_array_empty([0; CAPACITY_BYTES]) }
    }

    /// Creates a new `PackedNibbles` instance from an iterator of nibbles.
    ///
    /// Each item in the iterator should be a nibble (0-15).
    pub fn from_nibbles(nibbles: impl IntoIterator<Item = u8>) -> Self {
        let mut packed = Self::default();
        // TODO: this can be optimized
        for nibble in nibbles {
            packed.push_unchecked(nibble);
        }
        packed
    }

    /// Creates a new `PackedNibbles` instance from an iterator of nibbles without checking bounds.
    ///
    /// Each item in the iterator should be a nibble (0-15).
    /// This function is essentially identical to `from_nibbles` but is kept for API compatibility.
    pub fn from_nibbles_unchecked(nibbles: impl IntoIterator<Item = u8>) -> Self {
        let mut packed = Self::default();
        // TODO: this can be optimized
        for nibble in nibbles {
            packed.push_unchecked(nibble);
        }
        packed
    }

    /// Creates a new `PackedNibbles` instance from a slice of bytes.
    ///
    /// This treats each byte as a single element rather than unpacking into nibbles.
    pub fn unpack(bytes: impl AsRef<[u8]>) -> Self {
        Self {
            // TODO: this can be optimized
            nibbles: bytes.as_ref().iter().copied().collect(),
            even: bytes.as_ref().len() % 2 == 0,
        }
    }

    /// Returns `true` if this [`PackedNibbles`] is empty.
    pub fn is_empty(&self) -> bool {
        self.nibbles.is_empty()
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

        // Compare all bytes of other except the last one
        let other_last = other.nibbles.len() - 1;
        if other_last > 0 && self.nibbles[..other_last] != other.nibbles[..other_last] {
            return false;
        }

        // All bytes except the last one are equal, so we need to compare the last byte
        // of other
        let self_last = self.nibbles[other_last];
        let other_last = other.nibbles[other_last];
        if other.even {
            // If other is even, we just need to compare the last byte to self
            self_last == other_last
        } else {
            // If other is odd, and we're at its last byte, we need to compare only
            // the first nibble of both bytes
            self_last & 0xf0 == other_last & 0xf0
        }
    }

    /// Returns the total number of nibbles in this [`PackedNibbles`].
    pub fn len(&self) -> usize {
        if self.even {
            self.nibbles.len() * 2
        } else {
            self.nibbles.len() * 2 - 1
        }
    }

    /// Returns a slice of the underlying bytes.
    pub fn as_slice(&self) -> &[u8] {
        self.nibbles.as_slice()
    }

    /// Returns the length of the common prefix between this [`PackedNibbles`] and `other`.
    pub fn common_prefix_length(&self, other: &Self) -> usize {
        let len = core::cmp::min(self.len(), other.len());

        // TODO: this can be optimized

        // Compare each nibble individually until a mismatch is found
        for i in 0..len {
            if self.get_nibble(i) != other.get_nibble(i) {
                return i;
            }
        }

        // If no mismatch is found, return the minimum length
        len
    }

    /// Returns the last nibble in this [`PackedNibbles`], or `None` if empty.
    pub fn last(&self) -> Option<u8> {
        if self.is_empty() {
            return None;
        }

        Some(self.get_nibble(self.len() - 1))
    }

    /// Gets the nibble at the given position.
    ///
    /// For even positions (0, 2, 4...), returns the high nibble of the corresponding byte.
    /// For odd positions (1, 3, 5...), returns the low nibble of the corresponding byte.
    ///
    /// # Panics
    ///
    /// Panics if the position is out of bounds.
    pub fn get_nibble(&self, pos: usize) -> u8 {
        assert!(pos < self.len(), "position {} out of bounds (len: {})", pos, self.len());

        let byte_pos = pos / 2;
        // SAFETY: We have asserted that the position is within the current length.
        let byte = unsafe { self.nibbles.get_unchecked(byte_pos) };

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
        // Determine the start and end nibbl indices from the range bounds
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
            return self.clone();
        }

        let nibble_count = end - start;

        // Calculate starting and ending byte indices. Both indices include full bytes in case if
        // `start` or `end` are odd.
        //
        // For example, for byte representation `[0x12, 0x34]` and `start = 1, end = 2` this
        // will calculate `from_byte = 0` and `to_byte = 1`, and the sliced nibbles after patching
        // below will be represented as `[0x23]`.
        let from_byte = start / 2;
        let to_byte = end.div_ceil(2);

        // Calculate the byte count that needs to be allocated for the sliced nibbles **BEFORE** the
        // patching below is done.
        //
        // For the example above, this will calculate `byte_count = 2` to be able to store `[0x12,
        // 0x34]`, and then do additional patching.
        let byte_count = to_byte - from_byte;

        let mut out = ArrayVec::from_array_empty([0; CAPACITY_BYTES]);
        unsafe {
            // SAFETY: We fill `out` with `byte_count` elements below.
            out.set_len(byte_count);
            // SAFETY: `out` is a valid contiguous slice of length
            // `CAPACITY_BYTES` non-overlapping with `self.nibbles`.
            core::ptr::copy_nonoverlapping(
                self.nibbles.as_ptr().add(from_byte),
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

        // If we end on an odd nibble, we need to mask the last byte, so it has only the high nibble
        // set.
        //
        // For `out = [0x12, 0x34]`, it will be turned into `[0x12, 0x30]`.
        if end_odd {
            // SAFETY: access is within bounds, because `out` is initialized with exactly
            // `byte_count` bytes.
            let last = unsafe { out.get_unchecked_mut(byte_count - 1) };
            *last &= 0xF0;
        }

        // SAFETY: We've already checked that the length is valid, and we're setting the length
        // to a smaller value.
        out.set_len(nibble_count.div_ceil(2));

        Self { nibbles: out, even: (end - start) % 2 == 0 }
    }

    /// Extends this [`PackedNibbles`] with the given [`PackedNibbles`].
    pub fn extend_path(&mut self, other: &Self) {
        if other.is_empty() {
            // If `other` is empty, we can just return
            return;
        }

        if self.nibbles.is_empty() || self.even {
            // If we're empty or even, we can just extend the nibbles with `other` and update the
            // even flag
            self.nibbles.extend_from_slice(&other.nibbles);
            self.even = other.even;
            return;
        }

        // We're odd, so we need to merge nibbles one by one
        let prev_len = self.nibbles.len();

        // Reserve space for the new nibbles
        let new_len = if other.even {
            prev_len + other.nibbles.len()
        } else {
            prev_len + other.nibbles.len() - 1
        };
        assert!(new_len <= CAPACITY_BYTES);
        self.nibbles.set_len(new_len);

        // Merge the boundary.
        //
        // Example: `self` ends with [0x30] (representing "3") and `other` starts with [0x45]
        // (representing "45").
        // 1. Take first byte from `other`: 0x45
        // 2. Mask high nibble:             0x45 & 0xF0 = 0x40
        // 3. Shift right by 4:             0x40 >> 4   = 0x04
        // 4. Take last byte from `self`:   0x30
        // 5. Combine with OR:              0x30 | 0x04 = 0x34
        //
        // Result: self ends with [0x12, 0x34] and we continue merging the rest
        self.nibbles[prev_len - 1] |= (other.nibbles[0] & 0xF0) >> 4;

        // Process the rest of the bytes from `other` by shifting the nibbles to the right.
        //
        // Example: pushing bytes [0x45, 0x60] from `other` into the correct positions in `self`.
        //
        // For the first iteration:
        // 1. current = 0x45, next = 0x60
        // 2. Shift current low nibble left by 4:   (0x45 & 0x0F) << 4 = 0x50
        // 3. Shift next high nibble right by 4:    (0x60 & 0xF0) >> 4 = 0x06
        // 4. Combine with OR:                      0x50 | 0x06 = 0x56
        // 5. Place at the next position in `self`: self.nibbles[prev_len + 0] = 0x56
        for i in 0..other.nibbles.len() - 1 {
            let current = other.nibbles[i];
            let next = other.nibbles[i + 1];

            let high_nibble = (current & 0x0F) << 4;
            let low_nibble = (next & 0xF0) >> 4;

            self.nibbles[prev_len + i] = high_nibble | low_nibble;
        }

        // Handle the last byte based on whether `other` is even or odd.
        if other.even {
            // If `other` is even, the resulting `self` will be odd, so we need to get the high
            // nibble of the last byte of `other` and set it to the low nibble of the last byte of
            // `self`.
            //
            // Example: `other` ends with [0x60] (representing "60"):
            // 1. Mask low nibble:  0x60 & 0x0F = 0x00
            // 2. Shift left by 4:  0x00 << 4   = 0x00
            //
            // If `other` ends with [0x67] (representing "67"):
            // 1. Mask low nibble:  0x67 & 0x0F = 0x07
            // 2. Shift left by 4:  0x07 << 4   = 0x70
            let last = prev_len + other.nibbles.len() - 1;
            let low_nibble = (other.nibbles[other.nibbles.len() - 1] & 0x0F) << 4;

            self.nibbles[last] = low_nibble;
        }

        // We know that `self.even` is false, so we only set the even flag if both nibbles started
        // out with odd lengths.
        //
        // For this it's enough to check if the other had odd length.
        self.even = !other.even;
    }

    /// Pushes a single nibble to the end of the nibbles.
    ///
    /// This will only look at the low nibble of the byte passed, i.e. for `0x02` the nibble `2`
    /// will be pushed.
    ///
    /// NOTE: if there is data in the high nibble, it will be ignored.
    pub fn push_unchecked(&mut self, nibble: u8) {
        // If we're empty, we can just push the nibble
        if self.nibbles.is_empty() {
            self.nibbles.push(nibble << 4);
            self.even = false;
            return;
        }

        // We determine whether or not we should push a new high nibble or set the low nibble of
        // the last byte based on the even value
        if self.even {
            // Push a new byte with the nibble in the high position
            self.nibbles.push(nibble << 4);
        } else {
            let last = self.nibbles.len() - 1;

            // Set the low nibble of the last byte
            self.nibbles[last] |= nibble & 0x0F;
        }

        // Finally set the even / odd to the opposite of the current value
        self.even = !self.even;
    }

    /// Truncates this [`PackedNibbles`] to the specified length.
    ///
    /// This method will truncate the underlying byte array and update the `even` flag
    /// to match the new length.
    ///
    /// The provided length is the number of nibbles to keep, not the number of bytes.
    ///
    /// # Panics
    ///
    /// This method will panic if `new_len` is greater than the current nibble count.
    pub fn truncate(&mut self, new_len: usize) {
        // First calculate the actual nibble count we have
        let current_nibble_count =
            if self.even { self.nibbles.len() * 2 } else { self.nibbles.len() * 2 - 1 };

        assert!(
            new_len <= current_nibble_count,
            "new_len {} is greater than current length {}",
            new_len,
            current_nibble_count
        );

        // Calculate new array length - ceiling division for odd lengths
        let new_array_len = new_len.div_ceil(2);

        // Update even flag based on the new length
        self.even = new_len % 2 == 0;

        // If we're keeping only a subset of the bytes, truncate the array
        if new_array_len < self.nibbles.len() {
            self.nibbles.truncate(new_array_len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        // Test with different `even` flag values that have the same byte representation
        let even_nibbles = PackedNibbles::from_nibbles([1, 2, 3, 0]);
        let odd_nibbles = PackedNibbles::from_nibbles([1, 2, 3]);
        assert!(even_nibbles.even);
        assert!(!odd_nibbles.even);
        assert_eq!(even_nibbles.nibbles.as_slice(), [0x12, 0x30]);
        assert_eq!(odd_nibbles.nibbles.as_slice(), [0x12, 0x30]);
        assert_eq!(even_nibbles.cmp(&odd_nibbles), core::cmp::Ordering::Greater);
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
        assert!(even.even);

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
        assert!(!odd.even);

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
}
