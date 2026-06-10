//! Rayon parallel iterator utilities.

use alloc::vec::Vec;
use rayon::iter::IntoParallelIterator;

/// Extension trait for iterators to convert them to parallel iterators via collection.
///
/// This is an alternative to [`rayon::iter::ParallelBridge`] that first collects the iterator
/// into a `Vec`, then calls [`IntoParallelIterator`] on it. This avoids the mutex contention
/// that can occur with `par_bridge` when either the iterator's `next()` method is fast or the
/// parallel tasks are fast, as `par_bridge` wraps the iterator in a mutex.
///
/// # When to use
///
/// Use `par_bridge_buffered` instead of `par_bridge` when:
/// - The iterator produces items quickly
/// - The parallel work per item is relatively light
/// - The total number of items is known to be reasonable for memory
///
/// Stick with `par_bridge` when:
/// - The iterator is slow (e.g., I/O bound) and you want to overlap iteration with processing
/// - Memory is constrained and you cannot afford to collect all items upfront
pub trait ParallelBridgeBuffered: Iterator<Item: Send> + Sized {
    /// Collects this iterator into a `Vec` and returns a parallel iterator over it.
    ///
    /// See [this trait's documentation](ParallelBridgeBuffered) for more details.
    fn par_bridge_buffered(self) -> rayon::vec::IntoIter<Self::Item> {
        self.collect::<Vec<_>>().into_par_iter()
    }
}

impl<I: Iterator<Item: Send>> ParallelBridgeBuffered for I {}
