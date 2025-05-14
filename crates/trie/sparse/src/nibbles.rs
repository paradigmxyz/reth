use core::{
    fmt,
    ops::{Bound, RangeBounds},
    slice,
};

use arrayvec::ArrayVec;
use reth_trie_common::Nibbles;

/// A representation for nibbles, that uses an even/odd flag.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PackedNibbles {
    /// The nibbles themselves, stored as a byte array.
    pub nibbles: ArrayVec<u8, 32>,
    /// The even/odd flag, indicating whether the length is even or odd.
    pub even: bool,
}

impl Default for PackedNibbles {
    fn default() -> Self {
        Self { nibbles: ArrayVec::new(), even: true }
    }
}

impl PackedNibbles {
    pub fn unpack(bytes: impl AsRef<[u8]>) -> Self {
        Self {
            nibbles: ArrayVec::from_iter(bytes.as_ref().iter().copied()),
            even: bytes.as_ref().len() % 2 == 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nibbles.is_empty()
    }

    /// Returns `true` if this `PackedNibbles` starts with the nibbles in `other`.
    ///
    /// This method correctly handles even/odd nibble sequences and compares the
    /// actual nibble values, not just the byte representation.
    ///
    /// # Examples
    ///
    /// ```
    /// use crate::PackedNibbles;
    ///
    /// let nibbles1 = PackedNibbles::from_nibbles(&[0x0A, 0x0B, 0x0C, 0x0D]);
    /// let nibbles2 = PackedNibbles::from_nibbles(&[0x0A, 0x0B]);
    /// let nibbles3 = PackedNibbles::from_nibbles(&[0x0A, 0x0C]);
    ///
    /// assert!(nibbles1.starts_with(&nibbles2));
    /// assert!(!nibbles1.starts_with(&nibbles3));
    /// ```
    pub fn starts_with(&self, other: &Self) -> bool {
        // If other is empty, it's a prefix of any sequence
        if other.is_empty() {
            return true;
        }

        // If other is longer than self, it can't be a prefix
        let other_len = other.len();
        let self_len = self.len();

        if other_len > self_len {
            return false;
        }

        // Compare each nibble individually
        for i in 0..other_len {
            if self.get_nibble(i) != other.get_nibble(i) {
                return false;
            }
        }

        true
    }

    /// Returns the total number of nibbles in this `PackedNibbles`.
    ///
    /// This accounts for the `even` flag - for odd-length nibbles, the
    /// returned count is 2 * byte_count - 1.
    pub fn len(&self) -> usize {
        if self.even {
            self.nibbles.len() * 2
        } else {
            self.nibbles.len() * 2 - 1
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        self.nibbles.as_slice()
    }

    /// Returns the length of the common prefix between this `PackedNibbles` and `other`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crate::PackedNibbles;
    ///
    /// // Create nibbles with values [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F]
    /// let a = PackedNibbles::unpack(&[0xAB, 0xCD, 0xEF]);
    /// // Create nibbles with values [0x0A, 0x0B, 0x0C, 0x0D]
    /// let b = PackedNibbles::unpack(&[0xAB, 0xCD]);
    /// assert_eq!(a.common_prefix_length(&b), 4); // Common prefix is [0x0A, 0x0B, 0x0C, 0x0D]
    /// ```
    pub fn common_prefix_length(&self, other: &Self) -> usize {
        let len = core::cmp::min(self.len(), other.len());

        // Compare each nibble individually until a mismatch is found
        for i in 0..len {
            if self.get_nibble(i) != other.get_nibble(i) {
                return i;
            }
        }

        // If no mismatch is found, return the minimum length
        len
    }

    /// Returns the last nibble in this `PackedNibbles`, or `None` if empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crate::PackedNibbles;
    ///
    /// let nibbles = PackedNibbles::from_nibbles(&[0x0A, 0x0B, 0x0C]);
    /// assert_eq!(nibbles.last(), Some(0x0C));
    ///
    /// let empty = PackedNibbles::default();
    /// assert_eq!(empty.last(), None);
    /// ```
    pub fn last(&self) -> Option<u8> {
        if self.is_empty() {
            return None;
        }

        let len = self.len();
        Some(self.get_nibble(len - 1))
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
        let byte = self.nibbles[byte_pos];

        if pos % 2 == 0 {
            // For even positions, return the high nibble
            (byte & 0xF0) >> 4
        } else {
            // For odd positions, return the low nibble
            byte & 0x0F
        }
    }

    /// Creates a new `PackedNibbles` containing the nibbles in the specified range.
    ///
    /// This method accepts any type that implements `RangeBounds<usize>`,
    /// including full ranges (`..`), ranges with one bound (`..end` or `start..`),
    /// inclusive ranges (`start..=end`), or standard ranges (`start..end`).
    ///
    /// # Examples
    ///
    /// ```
    /// use crate::PackedNibbles;
    ///
    /// let original = PackedNibbles::from_nibbles(&[0x0A, 0x0B, 0x0C, 0x0D]);
    ///
    /// // Using a standard range
    /// let sliced = original.slice(1..3);
    /// assert_eq!(sliced.get_nibble(0), 0x0B);
    /// assert_eq!(sliced.get_nibble(1), 0x0C);
    /// assert_eq!(sliced.len(), 2);
    ///
    /// // Using an inclusive range
    /// let sliced = original.slice(1..=2);
    /// assert_eq!(sliced.get_nibble(0), 0x0B);
    /// assert_eq!(sliced.get_nibble(1), 0x0C);
    ///
    /// // Using a full range
    /// let sliced = original.slice(..);
    /// assert_eq!(sliced.len(), original.len());
    /// ```
    ///
    /// # Panics
    ///
    /// This method will panic if the range is out of bounds for this `PackedNibbles`.
    #[inline]
    pub fn slice<R>(&self, range: R) -> Self
    where
        R: RangeBounds<usize>,
    {
        // Determine the start and end indices from the range bounds
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

        // Check bounds
        assert!((start <= end), "slice start ({}) > end ({})", start, end);

        assert!(start <= len && end <= len, "slice bounds out of range (len: {})", len);

        // Fast path for empty slice
        if start == end {
            return Self { nibbles: ArrayVec::new(), even: true };
        }

        // Simple case - if we're slicing the whole thing
        if start == 0 && end == len {
            return self.clone();
        }

        // Create a new array to hold the sliced nibbles
        let slice_len = end - start;
        let mut result = Self { nibbles: ArrayVec::new(), even: slice_len % 2 == 0 };

        // Copy nibbles one at a time
        for i in 0..slice_len {
            let nibble = self.get_nibble(start + i);
            result.push_unchecked(nibble);
        }

        result
    }

    /// Extends this [`PackedNibbles`] with the given [`PackedNibbles`].
    pub fn extend_path(&mut self, other: &PackedNibbles) {
        if other.is_empty() {
            // If `other` is empty, we can just return
            return
        }

        if self.nibbles.is_empty() {
            // If we're empty, we can just use the `other` nibbles
            self.nibbles.copy_from_slice(&other.nibbles);
            self.even = other.even;
            return
        }

        if self.even {
            // If we're even, we can just extend the nibbles with `other` and update the even flag
            self.nibbles.try_extend_from_slice(&other.nibbles).expect("nibbles length is 32");
            self.even = other.even;
            return
        }

        // We're odd, so we need to merge nibbles one by one
        let prev_len = self.nibbles.len();

        // Reserve space for the new nibbles
        let new_len = if other.even {
            prev_len + other.nibbles.len()
        } else {
            prev_len + other.nibbles.len() - 1
        };
        unsafe { self.nibbles.set_len(new_len) };

        // Merge the boundary.
        //
        // Example: `self` ends with [0x30] (representing "3") and `other` starts with [0x45]
        // (representing "45").
        // 1. Take first byte from `other`: 0x45
        // 2. Mask low nibble:              0x45 & 0xF0 = 0x40
        // 3. Shift right by 4:             0x40 >> 4   = 0x04
        // 4. Take last byte from `self`:   0x30
        // 5. Combine with OR:              0x30 | 0x04 = 0x34
        //
        // Result: self ends with [0x12, 0x34] and we continue merging the rest
        self.nibbles[prev_len - 1] |= (other.nibbles[0] & 0xF0) >> 4;

        // Process the rest of the bytes from `other`.
        //
        // Example: pushing bytes [0x45, 0x60] from `other` into the correct positions in `self`.
        //
        // For the first iteration:
        // 1. current = 0x45, next = 0x60
        // 2. Shift current left by 4:              0x45 << 4   = 0x50
        // 3. Mask high nibble of next:             0x60 & 0xF0 = 0x60
        // 4. Shift right by 4:                     0x60 >> 4   = 0x06
        // 5. Combine with OR:                      0x50 | 0x06 = 0x56
        // 6. Place at the next position in `self`: self.nibbles[prev_len + 0] = 0x56
        for i in 0..other.nibbles.len() - 1 {
            let current = other.nibbles[i];
            let next = other.nibbles[i + 1];

            let low_nibble = current << 4;
            let high_nibble = (next & 0xF0) >> 4;

            self.nibbles[prev_len + i] = low_nibble | high_nibble;
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
    /// This will only look at the high nibble of the byte passed, i.e. for `0x02` the nibble `2`
    /// will be pushed.
    ///
    /// NOTE: if there is data in the low nibble, it will be ignored.
    pub fn push_unchecked(&mut self, nibble: u8) {
        // If we're empty, we can just push the nibble
        if self.nibbles.is_empty() {
            self.nibbles.push(nibble << 4);
            self.even = false;
            return
        }

        // We determine whether or not we should push a new low nibble or set the high nibble of
        // the last byte based on the even value
        if self.even {
            // Push a new low nibble
            self.nibbles.push(nibble << 4);
        } else {
            let last = self.nibbles.len() - 1;

            // Set the high nibble of the last byte
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
    /// # Examples
    ///
    /// ```
    /// use crate::PackedNibbles;
    ///
    /// let mut nibbles = PackedNibbles::from_nibbles(&[0x0A, 0x0B, 0x0C, 0x0D]);
    /// nibbles.truncate(2);
    /// assert_eq!(nibbles.nibbles.len(), 1);
    /// assert_eq!(nibbles.even, true);
    /// ```
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

impl From<Nibbles> for PackedNibbles {
    fn from(nibbles: Nibbles) -> Self {
        let mut packed = Self::default();
        // TODO: this is inefficient
        for b in nibbles.iter().copied() {
            packed.push_unchecked(b);
        }
        packed
    }
}

impl From<PackedNibbles> for Nibbles {
    fn from(packed: PackedNibbles) -> Self {
        // TODO: this is not correct for odd number of nibbles
        Self::unpack(packed.nibbles)
    }
}

impl PartialOrd for PackedNibbles {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackedNibbles {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        // First compare the nibbles arrays
        let nibbles_ordering = self.nibbles.cmp(&other.nibbles);

        // If the nibbles are equal, use the even flag to break the tie
        // When even flags are the same, the ordering is already determined by nibbles
        // When even flags differ, we need special handling
        if nibbles_ordering == core::cmp::Ordering::Equal && self.even != other.even {
            // For two otherwise identical sequences, the even one is "less than" the odd one
            // because the odd one has an implicit trailing 0 nibble
            if self.even {
                core::cmp::Ordering::Less
            } else {
                core::cmp::Ordering::Greater
            }
        } else {
            nibbles_ordering
        }
    }
}
