//! Incremental ordered trie root computation.
//!
//! This module provides builders for computing ordered trie roots incrementally as items
//! arrive, rather than requiring all items upfront. This is useful for receipt root
//! calculation during block execution, where we know the total count but receive receipts
//! one by one as transactions are executed.

use crate::{HashBuilder, Nibbles, EMPTY_ROOT_HASH};
use alloc::vec::Vec;
use alloy_primitives::B256;
use alloy_trie::root::adjust_index_for_rlp;
use core::fmt;

/// Error returned when using [`OrderedTrieRootEncodedBuilder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderedRootError {
    /// Called `finalize()` before all items were pushed.
    Incomplete {
        /// The expected number of items.
        expected: usize,
        /// The number of items received.
        received: usize,
    },
    /// Index is out of bounds.
    IndexOutOfBounds {
        /// The index that was provided.
        index: usize,
        /// The expected length.
        len: usize,
    },
    /// Item at this index was already pushed.
    DuplicateIndex {
        /// The duplicate index.
        index: usize,
    },
}

impl OrderedRootError {
    /// Returns `true` if the error is [`OrderedRootError::Incomplete`].
    #[inline]
    pub const fn is_incomplete(&self) -> bool {
        matches!(self, Self::Incomplete { .. })
    }

    /// Returns `true` if the error is [`OrderedRootError::IndexOutOfBounds`].
    #[inline]
    pub const fn is_index_out_of_bounds(&self) -> bool {
        matches!(self, Self::IndexOutOfBounds { .. })
    }

    /// Returns `true` if the error is [`OrderedRootError::DuplicateIndex`].
    #[inline]
    pub const fn is_duplicate_index(&self) -> bool {
        matches!(self, Self::DuplicateIndex { .. })
    }

    /// Returns the index associated with the error, if any.
    #[inline]
    pub const fn index(&self) -> Option<usize> {
        match self {
            Self::Incomplete { .. } => None,
            Self::IndexOutOfBounds { index, .. } | Self::DuplicateIndex { index } => Some(*index),
        }
    }
}

impl fmt::Display for OrderedRootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomplete { expected, received } => {
                write!(f, "incomplete: expected {expected} items, received {received}")
            }
            Self::IndexOutOfBounds { index, len } => {
                write!(f, "index {index} out of bounds for length {len}")
            }
            Self::DuplicateIndex { index } => {
                write!(f, "duplicate item at index {index}")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for OrderedRootError {}

/// A builder for computing ordered trie roots incrementally from pre-encoded items.
///
/// This builder allows pushing items one by one as they become available
/// (e.g., receipts after each transaction execution), rather than requiring
/// all items upfront.
///
/// # Use Case
///
/// When executing a block, the receipt root must be computed from all transaction
/// receipts. With the standard `ordered_trie_root`, you must wait until all
/// transactions are executed before computing the root. This builder enables
/// **incremental computation** - you can start building the trie as soon as
/// receipts become available, potentially in parallel with continued execution.
///
/// The builder requires knowing the total item count upfront (the number of
/// transactions in the block), but items can be pushed in any order by index.
///
/// # How It Works
///
/// Items can be pushed in any order by specifying their index. The builder
/// internally buffers items and flushes them to the underlying [`HashBuilder`]
/// in the correct order for RLP key encoding (as determined by [`adjust_index_for_rlp`]).
///
/// # Memory
///
/// Each pushed item is stored in an internal buffer until it can be flushed.
/// In the worst case (e.g., pushing index 0 last), all items except one will
/// be buffered. For receipt roots, index 0 is typically flushed late due to
/// RLP key ordering, so expect to buffer most items until near the end.
///
/// # Example
///
/// ```
/// use reth_trie_common::ordered_root::OrderedTrieRootEncodedBuilder;
///
/// // Create a builder for 2 pre-encoded items
/// let mut builder = OrderedTrieRootEncodedBuilder::new(2);
///
/// // Push pre-encoded items as they arrive (can be out of order)
/// builder.push(1, b"encoded_item_1").unwrap();
/// builder.push(0, b"encoded_item_0").unwrap();
///
/// // Finalize to get the root hash
/// let root = builder.finalize().unwrap();
/// ```
#[derive(Debug)]
pub struct OrderedTrieRootEncodedBuilder {
    /// Total expected number of items.
    len: usize,
    /// Number of items received so far.
    received: usize,
    /// Next insertion loop counter (determines which adjusted index to flush next).
    next_insert_i: usize,
    /// Buffer for pending items, indexed by execution index.
    pending: Vec<Option<Vec<u8>>>,
    /// The underlying hash builder.
    hb: HashBuilder,
}

impl OrderedTrieRootEncodedBuilder {
    /// Creates a new builder for `len` pre-encoded items.
    pub fn new(len: usize) -> Self {
        Self {
            len,
            received: 0,
            next_insert_i: 0,
            pending: alloc::vec![None; len],
            hb: HashBuilder::default(),
        }
    }

    /// Pushes a pre-encoded item at the given index to the builder.
    ///
    /// Items can be pushed in any order. The builder will automatically
    /// flush items to the underlying [`HashBuilder`] when they become
    /// available in the correct order.
    ///
    /// # Errors
    ///
    /// - [`OrderedRootError::IndexOutOfBounds`] if `index >= len`
    /// - [`OrderedRootError::DuplicateIndex`] if an item was already pushed at this index
    #[inline]
    pub fn push(&mut self, index: usize, bytes: &[u8]) -> Result<(), OrderedRootError> {
        if index >= self.len {
            return Err(OrderedRootError::IndexOutOfBounds { index, len: self.len });
        }

        if self.pending[index].is_some() {
            return Err(OrderedRootError::DuplicateIndex { index });
        }

        self.push_unchecked(index, bytes);
        Ok(())
    }

    /// Pushes a pre-encoded item at the given index without bounds or duplicate checking.
    ///
    /// This is a performance-critical method for callers that can guarantee:
    /// - `index < len`
    /// - No item has been pushed at this index before
    ///
    /// # Panics
    ///
    /// Panics in debug mode if `index >= len`.
    #[inline]
    pub fn push_unchecked(&mut self, index: usize, bytes: &[u8]) {
        debug_assert!(index < self.len, "index {index} out of bounds for length {}", self.len);
        debug_assert!(self.pending[index].is_none(), "duplicate item at index {index}");

        self.pending[index] = Some(bytes.to_vec());
        self.received += 1;

        self.flush();
    }

    /// Attempts to flush pending items to the hash builder.
    fn flush(&mut self) {
        while self.next_insert_i < self.len {
            let exec_index_needed = adjust_index_for_rlp(self.next_insert_i, self.len);

            let Some(value) = self.pending[exec_index_needed].take() else {
                break;
            };

            let index_buffer = alloy_rlp::encode_fixed_size(&exec_index_needed);
            self.hb.add_leaf(Nibbles::unpack(&index_buffer), &value);

            self.next_insert_i += 1;
        }
    }

    /// Returns `true` if all items have been pushed.
    #[inline]
    pub const fn is_complete(&self) -> bool {
        self.received == self.len
    }

    /// Returns the number of items pushed so far.
    #[inline]
    pub const fn pushed_count(&self) -> usize {
        self.received
    }

    /// Returns the expected total number of items.
    #[inline]
    pub const fn expected_count(&self) -> usize {
        self.len
    }

    /// Finalizes the builder and returns the trie root.
    ///
    /// # Errors
    ///
    /// Returns [`OrderedRootError::Incomplete`] if not all items have been pushed.
    pub fn finalize(mut self) -> Result<B256, OrderedRootError> {
        if self.len == 0 {
            return Ok(EMPTY_ROOT_HASH);
        }

        if self.received != self.len {
            return Err(OrderedRootError::Incomplete {
                expected: self.len,
                received: self.received,
            });
        }

        debug_assert_eq!(self.next_insert_i, self.len, "not all items were flushed");

        Ok(self.hb.root())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_trie::root::ordered_trie_root_encoded;

    #[test]
    fn test_ordered_encoded_builder_equivalence() {
        for len in [0, 1, 2, 3, 10, 127, 128, 129, 130, 200] {
            let items: Vec<Vec<u8>> =
                (0..len).map(|i| format!("item_{i}_data").into_bytes()).collect();

            let expected = ordered_trie_root_encoded(&items);

            let mut builder = OrderedTrieRootEncodedBuilder::new(len);

            for (i, item) in items.iter().enumerate() {
                builder.push(i, item).unwrap();
            }

            let actual = builder.finalize().unwrap();
            assert_eq!(
                expected, actual,
                "mismatch for len={len}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn test_ordered_builder_out_of_order() {
        for len in [2, 3, 5, 10, 50] {
            let items: Vec<Vec<u8>> =
                (0..len).map(|i| format!("item_{i}_data").into_bytes()).collect();

            let expected = ordered_trie_root_encoded(&items);

            // Push in reverse order
            let mut builder = OrderedTrieRootEncodedBuilder::new(len);
            for i in (0..len).rev() {
                builder.push(i, &items[i]).unwrap();
            }
            let actual = builder.finalize().unwrap();
            assert_eq!(expected, actual, "mismatch for reverse order len={len}");

            // Push odds first, then evens
            let mut builder = OrderedTrieRootEncodedBuilder::new(len);
            for i in (1..len).step_by(2) {
                builder.push(i, &items[i]).unwrap();
            }
            for i in (0..len).step_by(2) {
                builder.push(i, &items[i]).unwrap();
            }
            let actual = builder.finalize().unwrap();
            assert_eq!(expected, actual, "mismatch for odd/even order len={len}");
        }
    }

    #[test]
    fn test_ordered_builder_empty() {
        let builder = OrderedTrieRootEncodedBuilder::new(0);
        assert!(builder.is_complete());
        assert_eq!(builder.finalize().unwrap(), EMPTY_ROOT_HASH);
    }

    #[test]
    fn test_ordered_builder_incomplete_error() {
        let mut builder = OrderedTrieRootEncodedBuilder::new(3);

        builder.push(0, b"item_0").unwrap();
        builder.push(1, b"item_1").unwrap();

        assert!(!builder.is_complete());
        assert_eq!(
            builder.finalize(),
            Err(OrderedRootError::Incomplete { expected: 3, received: 2 })
        );
    }

    #[test]
    fn test_ordered_builder_index_errors() {
        let mut builder = OrderedTrieRootEncodedBuilder::new(2);

        assert_eq!(
            builder.push(5, b"item"),
            Err(OrderedRootError::IndexOutOfBounds { index: 5, len: 2 })
        );

        builder.push(0, b"item_0").unwrap();

        assert_eq!(
            builder.push(0, b"item_0_dup"),
            Err(OrderedRootError::DuplicateIndex { index: 0 })
        );

        builder.push(1, b"item_1").unwrap();
        assert!(builder.is_complete());
    }
}
