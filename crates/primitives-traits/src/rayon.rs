//! Rayon parallel iterator utilities.

use alloc::vec::Vec;
use rayon::iter::{IntoParallelIterator, ParallelIterator};

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

    /// Collects, maps each item with `f`, and returns results as a `Vec`.
    ///
    /// Uses sequential iteration when the item count is at or below `threshold`,
    /// avoiding rayon scheduling overhead (~10-15µs per call) for small collections.
    fn maybe_par_map_collect<R, F>(self, threshold: usize, f: F) -> Vec<R>
    where
        R: Send,
        F: Fn(Self::Item) -> R + Sync + Send,
    {
        let items: Vec<Self::Item> = self.collect();
        if items.len() <= threshold {
            items.into_iter().map(f).collect()
        } else {
            items.into_par_iter().map(f).collect()
        }
    }

    /// Collects, then calls `f` on each item either sequentially or in parallel.
    ///
    /// Uses sequential iteration when the item count is at or below `threshold`,
    /// avoiding rayon scheduling overhead (~10-15µs per call) for small collections.
    fn maybe_par_for_each<F>(self, threshold: usize, f: F)
    where
        F: Fn(Self::Item) + Sync + Send,
    {
        let items: Vec<Self::Item> = self.collect();
        if items.len() <= threshold {
            items.into_iter().for_each(f);
        } else {
            items.into_par_iter().for_each(f);
        }
    }
}

impl<I: Iterator<Item: Send>> ParallelBridgeBuffered for I {}
