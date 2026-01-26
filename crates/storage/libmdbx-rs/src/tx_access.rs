//! Transaction pointer access abstraction.
//!
//! This module provides the [`TxPtrAccess`] trait which abstracts over different ways
//! of accessing the underlying MDBX transaction pointer. This enables future support
//! for both synchronized (mutex-protected) and unsynchronized transaction types.
//!
//! # Design
//!
//! MDBX has strict requirements for transaction access:
//! - All operations on a transaction must be totally ordered and non-concurrent
//! - Read-write transactions can only be used from the thread that created them
//!
//! The current implementation uses a `Mutex` to enforce these requirements at runtime.
//! This is correct and safe, but the synchronization overhead adds up in hot paths.
//!
//! This trait enables future implementations that can enforce these requirements at
//! compile time instead (e.g., using `&mut self` for exclusive access), eliminating
//! the runtime overhead for single-threaded workloads.

use crate::error::Result;
use std::fmt;

mod sealed {
    use crate::transaction::TransactionPtr;

    pub trait Sealed {}
    impl Sealed for TransactionPtr {}
}

/// Trait for accessing the transaction pointer.
///
/// This trait abstracts over different ways transaction pointers are stored
/// and accessed. It enables both synchronized (mutex-protected) and
/// unsynchronized access patterns for transactions.
///
/// # Implementors
///
/// - [`TransactionPtr`](crate::transaction::TransactionPtr) - Synchronized access via mutex
///
/// # Future Extensions
///
/// This trait is designed to support future unsynchronized transaction types
/// that use `&mut self` to enforce exclusive access at compile time, avoiding
/// mutex overhead in single-threaded contexts.
pub trait TxPtrAccess: fmt::Debug + sealed::Sealed {
    /// Execute a closure with the transaction pointer.
    ///
    /// The closure receives a valid transaction pointer that can be used for
    /// MDBX operations. For synchronized implementations, this acquires the
    /// appropriate lock before executing the closure.
    ///
    /// # Errors
    ///
    /// Returns an error if the transaction has timed out or is otherwise
    /// invalid.
    fn with_txn_ptr<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> R;

    /// Execute a closure with the transaction pointer, attempting to renew
    /// the transaction if it has timed out.
    ///
    /// This is primarily used for cleanup operations (like closing cursors)
    /// that need to succeed even after a timeout. For implementations that
    /// don't support renewal, this falls back to [`with_txn_ptr`](Self::with_txn_ptr).
    fn with_txn_ptr_for_cleanup<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> R,
    {
        self.with_txn_ptr(f)
    }
}
