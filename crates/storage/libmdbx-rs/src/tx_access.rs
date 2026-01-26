//! Transaction pointer access abstraction.
//!
//! This module provides the [`TxPtrAccess`] trait which abstracts over different ways
//! of accessing the underlying MDBX transaction pointer. This enables support
//! for both synchronized (mutex-protected) and unsynchronized transaction types.
//!
//! # Design
//!
//! MDBX has strict requirements for transaction access:
//! - All operations on a transaction must be totally ordered and non-concurrent
//! - Read-write transactions can only be used from the thread that created them
//!
//! The synchronized implementation ([`crate::TransactionPtr`]) uses a `Mutex` to enforce
//! these requirements at runtime. This is correct and safe, but the synchronization
//! overhead adds up in hot paths (100 reads = 100 lock acquisitions).
//!
//! The unsynchronized implementations ([`RwUnsync`], [`RoUnsync`]) enforce these
//! requirements at compile time via ownership and `!Send`/`!Sync` bounds, eliminating
//! the runtime overhead for single-threaded workloads.

use crate::error::Result;
use std::{
    fmt,
    marker::PhantomData,
    sync::atomic::{AtomicBool, Ordering},
};

mod sealed {
    use crate::transaction::TransactionPtr;

    pub trait Sealed {}
    impl Sealed for TransactionPtr {}
    impl Sealed for super::RwUnsync {}
    impl Sealed for super::RoUnsync {}
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
/// - [`RwUnsync`] - Unsynchronized read-write access (`!Send + !Sync`)
/// - [`RoUnsync`] - Unsynchronized read-only access (`Send + !Sync`)
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

/// Unsynchronized read-write transaction pointer.
///
/// This type provides zero-overhead access to the transaction pointer for
/// single-threaded read-write workloads. It is `!Send` and `!Sync` to ensure
/// it stays on the creating thread, matching MDBX's requirements.
///
/// When dropped without committing, the transaction is aborted.
pub struct RwUnsync {
    ptr: *mut ffi::MDBX_txn,
    committed: AtomicBool,
    // Make !Send and !Sync
    _marker: PhantomData<*mut ()>,
}

impl fmt::Debug for RwUnsync {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RwUnsync")
            .field("committed", &self.committed.load(Ordering::Relaxed))
            .finish()
    }
}

impl RwUnsync {
    /// Creates a new unsynchronized read-write transaction pointer.
    pub(crate) const fn new(ptr: *mut ffi::MDBX_txn) -> Self {
        Self { ptr, committed: AtomicBool::new(false), _marker: PhantomData }
    }

    /// Marks the transaction as committed.
    pub(crate) fn set_committed(&self) {
        self.committed.store(true, Ordering::SeqCst);
    }

    /// Returns true if the transaction has been committed.
    pub(crate) fn is_committed(&self) -> bool {
        self.committed.load(Ordering::SeqCst)
    }
}

impl TxPtrAccess for RwUnsync {
    #[inline]
    fn with_txn_ptr<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> R,
    {
        Ok(f(self.ptr))
    }
}

impl Drop for RwUnsync {
    fn drop(&mut self) {
        if !self.is_committed() {
            unsafe {
                ffi::mdbx_txn_abort(self.ptr);
            }
        }
    }
}

/// Unsynchronized read-only transaction pointer.
///
/// This type provides zero-overhead access to the transaction pointer for
/// single-threaded read-only workloads. It is `Send` but `!Sync` - it can
/// be moved between threads but not shared concurrently.
///
/// When dropped, the transaction is aborted.
pub struct RoUnsync {
    ptr: *mut ffi::MDBX_txn,
    // Make !Sync but allow Send
    _marker: PhantomData<std::cell::Cell<()>>,
}

impl fmt::Debug for RoUnsync {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoUnsync").finish()
    }
}

impl RoUnsync {
    /// Creates a new unsynchronized read-only transaction pointer.
    pub(crate) const fn new(ptr: *mut ffi::MDBX_txn) -> Self {
        Self { ptr, _marker: PhantomData }
    }
}

impl TxPtrAccess for RoUnsync {
    #[inline]
    fn with_txn_ptr<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> R,
    {
        Ok(f(self.ptr))
    }
}

impl Drop for RoUnsync {
    fn drop(&mut self) {
        unsafe {
            ffi::mdbx_txn_abort(self.ptr);
        }
    }
}

// SAFETY: RoUnsync can be sent between threads (MDBX allows this for RO transactions)
// but cannot be shared concurrently (!Sync via PhantomData<Cell<()>>).
unsafe impl Send for RoUnsync {}
