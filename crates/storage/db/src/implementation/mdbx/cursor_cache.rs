//! Cursor cache for reusing database cursors across operations.
//!
//! This module provides a cache for MDBX cursors, allowing them to be reused
//! rather than recreated for each operation. This reduces overhead in
//! cursor-heavy workloads like state iteration.

use reth_libmdbx::ffi;
use std::cell::UnsafeCell;

/// A wrapper around a raw MDBX cursor pointer that implements `Send`.
///
/// # Safety
///
/// MDBX cursors can be moved between threads as long as the owning transaction
/// is also moved. Since `Tx` is `Send`, the cursor can be too. The cursor must
/// not be used after the transaction is dropped.
#[derive(Debug)]
struct SendableCursor(*mut ffi::MDBX_cursor);

// SAFETY: MDBX cursors can be safely sent between threads when moved with their transaction.
// The transaction owns the cursor and ensures it's valid.
unsafe impl Send for SendableCursor {}

/// Cache for raw MDBX cursor pointers.
///
/// Stores cursors as `(table_index, cursor_ptr)` pairs. Most transactions only use
/// a few tables, so a small Vec with linear search is more memory-efficient than
/// a large fixed-size array.
///
/// Cursors are returned to the cache on drop via [`super::cursor::Cursor`] and
/// reused on subsequent `cursor_read`/`cursor_write` calls.
///
/// # Safety
///
/// The cache stores raw pointers to MDBX cursors. These are only valid while
/// the parent transaction is alive. The cache must be dropped before the
/// transaction is dropped, which is guaranteed by field ordering in `Tx`.
///
/// This type uses `UnsafeCell` for interior mutability and manually implements `Sync`.
/// This is safe because:
/// - MDBX transactions are not actually shared across threads (they're `Send` but single-threaded)
/// - The `Sync` bound on `Database::TX` is a historical API requirement, not a real usage pattern
/// - All access to the cache happens on a single thread that owns the transaction
pub struct CursorCache {
    /// Cached cursors as `(table_index, cursor_ptr)` pairs.
    entries: UnsafeCell<Vec<(usize, SendableCursor)>>,
}

// SAFETY: While CursorCache uses UnsafeCell, it's only accessed from a single thread.
// The Sync implementation is required because Database::TX requires Sync, but in practice
// transactions are never shared across threads - they're moved between threads, not shared.
unsafe impl Sync for CursorCache {}

impl std::fmt::Debug for CursorCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorCache").finish_non_exhaustive()
    }
}

impl Default for CursorCache {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorCache {
    /// Creates a new empty cursor cache.
    pub const fn new() -> Self {
        Self { entries: UnsafeCell::new(Vec::new()) }
    }

    /// Takes a cached cursor for the given table index, if one exists.
    ///
    /// # Safety
    ///
    /// This must only be called from a single thread (the one owning the transaction).
    #[inline]
    pub fn take(&self, index: usize) -> Option<*mut ffi::MDBX_cursor> {
        // SAFETY: We only access this from a single thread (the transaction owner)
        let entries = unsafe { &mut *self.entries.get() };
        entries.iter().position(|(idx, _)| *idx == index).map(|pos| entries.swap_remove(pos).1 .0)
    }

    /// Stores a cursor in the cache for the given table index.
    ///
    /// If a cursor already exists for this index, the old cursor is returned
    /// and must be closed by the caller.
    ///
    /// # Safety
    ///
    /// This must only be called from a single thread (the one owning the transaction).
    #[inline]
    pub fn put(
        &self,
        index: usize,
        cursor: *mut ffi::MDBX_cursor,
    ) -> Option<*mut ffi::MDBX_cursor> {
        // SAFETY: We only access this from a single thread (the transaction owner)
        let entries = unsafe { &mut *self.entries.get() };
        // Check if there's already an entry for this index
        if let Some(pos) = entries.iter().position(|(idx, _)| *idx == index) {
            let old = std::mem::replace(&mut entries[pos].1, SendableCursor(cursor));
            Some(old.0)
        } else {
            entries.push((index, SendableCursor(cursor)));
            None
        }
    }
}

impl Drop for CursorCache {
    fn drop(&mut self) {
        // SAFETY: We have &mut self, so we have exclusive access
        let entries = self.entries.get_mut();
        // Close all cached cursors
        for (_, SendableCursor(cursor)) in entries.drain(..) {
            // SAFETY: We own these cursors and are dropping them before the transaction
            unsafe {
                ffi::mdbx_cursor_close(cursor);
            }
        }
    }
}
