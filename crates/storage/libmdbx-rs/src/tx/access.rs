use crate::{
    sys::txn_manager::{RawTxPtr, TxnManagerMessage},
    Environment, MdbxResult, TransactionKind,
};
use core::{fmt, marker::PhantomData};
use parking_lot::{Mutex, MutexGuard};
use std::{
    ops,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::sync_channel,
        Arc,
    },
};

mod sealed {
    use super::*;

    #[allow(unreachable_pub)]
    pub trait Sealed {}
    impl Sealed for super::RoGuard {}
    impl Sealed for super::RwUnsync {}
    impl<K: TransactionKind> Sealed for super::PtrSyncInner<K> {}
    impl<K: TransactionKind> Sealed for super::PtrSync<K> {}
}

/// Trait for accessing the transaction pointer.
///
/// This trait abstracts over the different ways transaction pointers
/// are stored for read-only and read-write transactions. It ensures that
/// the transaction pointer can be accessed safely, respecting timeouts
/// and ownership semantics.
#[allow(unreachable_pub)]
pub trait TxPtrAccess: fmt::Debug + sealed::Sealed {
    /// Execute a closure with the transaction pointer.
    fn with_txn_ptr<F, R>(&self, f: F) -> MdbxResult<R>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> R;

    /// Execute a closure with the transaction pointer, attempting to renew
    /// the transaction if it has timed out.
    ///
    /// This is primarily used for cleanup operations (like closing cursors)
    /// that need to succeed even after a timeout. For implementations that
    /// don't support renewal (like `RoGuard` after the Arc is dropped), this
    /// falls back to `with_txn_ptr`.
    fn with_txn_ptr_for_cleanup<F, R>(&self, f: F) -> MdbxResult<R>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> R,
    {
        // Default: just use the normal path
        self.with_txn_ptr(f)
    }

    /// Mark the transaction as committed.
    fn mark_committed(&self);
}

/// Wrapper for raw txn pointer for RW transactions.
pub struct RwUnsync {
    committed: AtomicBool,
    ptr: *mut ffi::MDBX_txn,
}

impl fmt::Debug for RwUnsync {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RwUnsync").field("committed", &self.committed).finish()
    }
}

impl RwUnsync {
    /// Create a new [`RwUnsync`].
    pub(crate) const fn new(ptr: *mut ffi::MDBX_txn) -> Self {
        Self { committed: AtomicBool::new(false), ptr }
    }
}

impl TxPtrAccess for RwUnsync {
    fn with_txn_ptr<F, R>(&self, f: F) -> MdbxResult<R>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> R,
    {
        Ok(f(self.ptr))
    }

    fn mark_committed(&self) {
        // SAFETY:
        // Type is neither Sync nor Send, so no concurrent access is possible.
        unsafe { *self.committed.as_ptr() = true };
    }
}

impl Drop for RwUnsync {
    fn drop(&mut self) {
        // SAFETY:
        // We have exclusive ownership of this pointer.
        unsafe {
            if !*self.committed.as_ptr() {
                ffi::mdbx_txn_abort(self.ptr);
            }
        }
    }
}

/// Wrapper for raw txn pointer that calls abort on drop.
///
/// Used by the timeout mechanism - when the Arc is dropped, the transaction
/// is aborted.
pub(crate) struct RoTxPtr {
    ptr: *mut ffi::MDBX_txn,
}

impl fmt::Debug for RoTxPtr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoTxPtr").finish()
    }
}

#[cfg(feature = "read-tx-timeouts")]
impl Drop for RoTxPtr {
    fn drop(&mut self) {
        // SAFETY:
        // We have exclusive ownership of this pointer.
        // This is guaranteed by the Arc mechanism in RoGuard.
        unsafe {
            ffi::mdbx_txn_abort(self.ptr);
        }
    }
}

impl From<*mut ffi::MDBX_txn> for RoTxPtr {
    fn from(txn: *mut ffi::MDBX_txn) -> Self {
        Self { ptr: txn }
    }
}

// SAFETY:
// The RO transaction can be sent between threads, but not shared. RO
// transactions are not Sync because operations must be serialized.
unsafe impl Send for RoTxPtr {}

// SAFETY
// Usage within this crate MUST ensure that RoTxPtr is not used concurrently.
// Implementing Sync here allows RoTxPtr to be held in the Arc that we use for
// timeouts.
unsafe impl Sync for RoTxPtr {}

#[cfg(feature = "read-tx-timeouts")]
type WeakRoTxPtr = std::sync::Weak<RoTxPtr>;

type PhantomUnsync = PhantomData<fn() -> std::cell::Cell<()>>;

/// Guard that keeps a RO transaction alive.
///
/// This type MUST NOT be Sync, to prevent concurrent use of the underlying RO
/// tx pointer.
pub struct RoGuard {
    /// Strong reference to keep the transaction alive.
    strong: Option<std::sync::Arc<RoTxPtr>>,

    /// Weak reference for timeout case.
    #[cfg(feature = "read-tx-timeouts")]
    weak: WeakRoTxPtr,

    /// Whether the transaction was committed.
    committed: AtomicBool,

    /// Marker to prevent Sync implementation.
    _unsync: PhantomUnsync,
}

impl fmt::Debug for RoGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoGuard").field("committed", &self.committed).finish()
    }
}

impl RoGuard {
    /// Create a new RoGuard with no timeout (we keep the Arc).
    ///
    /// # Warning
    ///
    /// RO transactions consume resources while open. Disabling the timeout
    /// without closing the transaction may lead to resource exhaustion if
    /// done excessively.
    #[cfg_attr(feature = "read-tx-timeouts", allow(dead_code))]
    pub(crate) fn new_no_timeout(ptr: RoTxPtr) -> Self {
        let arc = std::sync::Arc::new(ptr);

        #[cfg(feature = "read-tx-timeouts")]
        let weak = std::sync::Arc::downgrade(&arc);

        Self {
            strong: Some(arc),

            #[cfg(feature = "read-tx-timeouts")]
            weak,

            committed: AtomicBool::new(false),

            _unsync: PhantomData,
        }
    }

    /// Create a new RoGuard with a timeout. After the timeout, the transaction
    /// will be aborted (unless a [`Self::with_txn_ptr`] call is in progress).
    #[cfg(feature = "read-tx-timeouts")]
    pub(crate) fn new_with_timeout(ptr: RoTxPtr, duration: std::time::Duration) -> Self {
        let arc = std::sync::Arc::new(ptr);
        let weak = std::sync::Arc::downgrade(&arc);
        std::thread::spawn(move || {
            std::thread::sleep(duration);
            // Drop the Arc, aborting the transaction.
            drop(arc);
        });

        Self { strong: None, weak, committed: AtomicBool::new(false), _unsync: PhantomData }
    }

    /// Try to get a strong reference to the transaction pointer.
    pub(crate) fn try_ref(&self) -> Option<std::sync::Arc<RoTxPtr>> {
        // SAFETY:
        // Type is not Sync. So no concurrent access is possible.
        if unsafe { *self.committed.as_ptr() } {
            return None;
        }

        if let Some(strong) = &self.strong {
            return Some(strong.clone());
        }

        #[cfg(feature = "read-tx-timeouts")]
        {
            self.weak.upgrade()
        }

        #[cfg(not(feature = "read-tx-timeouts"))]
        {
            None
        }
    }

    /// Attempt to upgrade the weak reference to a strong one, disabling the
    /// timeout. On success, the transaction will remain valid until this guard
    /// is dropped.
    ///
    /// # Warning
    ///
    /// RO transactions consume resources while open. Disabling the timeout
    /// without closing the transaction may lead to resource exhaustion if
    /// done excessively.
    #[cfg(feature = "read-tx-timeouts")]
    pub(crate) fn try_disable_timer(&mut self) -> MdbxResult<()> {
        if self.strong.is_some() {
            return Ok(());
        }
        if let Some(arc) = self.weak.upgrade() {
            self.strong = Some(arc);
            return Ok(());
        }
        Err(crate::MdbxError::ReadTransactionTimeout)
    }
}

impl TxPtrAccess for RoGuard {
    /// Execute a closure with the transaction pointer, failing if timed out.
    ///
    /// Calling this function will ensure that the transaction is still valid
    /// until the closure returns. If the closure returns an error, it will be
    /// propagated.
    ///
    /// # Warnings
    ///
    /// The closure CAN NOT store the pointer or references derived from it, as
    /// they may become invalid if the transaction times out.
    ///
    /// The closure prevents the transaction from timing out while it is
    /// executing. The closure is expected to be short-lived to avoid holding
    /// open resources.
    ///
    /// The `&mut self` receiver ensures that concurrent calls to this method
    /// are not possible, preventing data races on the underlying transaction.
    /// This is a HARD REQUIREMENT for safety.
    fn with_txn_ptr<F, R>(&self, f: F) -> MdbxResult<R>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> R,
    {
        #[cfg(feature = "read-tx-timeouts")]
        {
            // Fast path: if we own it, use directly.
            // This is ALWAYS the case without timeouts.
            if let Some(strong) = self.try_ref() {
                return Ok(f(strong.ptr));
            }
            Err(crate::MdbxError::ReadTransactionTimeout)
        }

        #[cfg(not(feature = "read-tx-timeouts"))]
        {
            let Some(arc) = self.try_ref() else { unreachable!() };
            Ok(f(arc.ptr))
        }
    }

    /// Execute a cleanup closure, even if the transaction has timed out.
    ///
    /// When a transaction times out, the Arc is dropped and the transaction is
    /// aborted. However, cursors still need to be closed to free memory. This
    /// method ensures the cleanup closure runs regardless of timeout status.
    ///
    /// If the transaction has timed out, a null pointer is passed since the
    /// transaction no longer exists. Cleanup operations (like cursor close)
    /// don't actually need the transaction pointer.
    fn with_txn_ptr_for_cleanup<F, R>(&self, f: F) -> MdbxResult<R>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> R,
    {
        #[cfg(feature = "read-tx-timeouts")]
        {
            // If we can get the strong ref, use it normally.
            if let Some(strong) = self.try_ref() {
                return Ok(f(strong.ptr));
            }
            // Transaction timed out and was aborted. Still run cleanup with
            // null pointer - cursor close doesn't need a valid txn.
            Ok(f(std::ptr::null_mut()))
        }

        #[cfg(not(feature = "read-tx-timeouts"))]
        {
            // Without timeouts, we always have the Arc.
            let Some(arc) = self.try_ref() else { unreachable!() };
            Ok(f(arc.ptr))
        }
    }

    fn mark_committed(&self) {
        // SAFETY:
        // Type is not Sync. So no concurrent access is possible.
        unsafe { *self.committed.as_ptr() = true };
    }
}

/// A shareable, thread-safe pointer to an MDBX transaction.
pub(crate) struct PtrSync<K: TransactionKind> {
    inner: Arc<PtrSyncInner<K>>,
}

impl<K: TransactionKind> fmt::Debug for PtrSync<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PtrSync")
            .field("txn", &(self.inner.txn as usize))
            .field("committed", &self.inner.committed)
            .finish()
    }
}

impl<K: TransactionKind> ops::Deref for PtrSync<K> {
    type Target = PtrSyncInner<K>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<K: TransactionKind> Clone for PtrSync<K> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl<K: TransactionKind> PtrSync<K> {
    /// Create a new PtrSync.
    pub(crate) fn new(env: Environment, txn: *mut ffi::MDBX_txn) -> Self {
        Self { inner: Arc::new(PtrSyncInner::new(env, txn)) }
    }
}

/// A shareable pointer to an MDBX transaction.
///
/// This type is used internally to manage transaction access in the [`TxSync`]
/// transaction API. Users typically don't interact with this type directly.
///
/// [`TxSync`]: crate::tx::TxSync
#[derive(Debug)]
pub struct PtrSyncInner<K: TransactionKind> {
    /// Raw pointer to the MDBX transaction.
    txn: *mut ffi::MDBX_txn,

    /// Whether the transaction was committed.
    committed: AtomicBool,

    /// Contains a lock to ensure exclusive access to the transaction.
    /// The inner boolean indicates the timeout status.
    lock: Mutex<bool>,

    /// The environment that owns the transaction.
    env: Environment,

    /// Tracing span for this transaction's lifecycle.
    span: tracing::Span,

    /// Marker for the transaction kind.
    _marker: PhantomData<fn() -> K>,
}

impl<K: TransactionKind> PtrSyncInner<K> {
    /// Create a new PtrSyncInner.
    pub(crate) fn new(env: Environment, txn: *mut ffi::MDBX_txn) -> Self {
        // Record txn_id after creation
        let txn_id = unsafe { ffi::mdbx_txn_id(txn) };

        let span = tracing::debug_span!(
            target: "libmdbx",
            "mdbx_txn",
            kind = %if K::IS_READ_ONLY { "ro" } else { "rw" },
            txn_id = tracing::field::Empty,
        );
        span.record("txn_id", txn_id);

        Self {
            txn,
            committed: AtomicBool::new(false),
            lock: Mutex::new(false),
            env,
            _marker: PhantomData,
            span,
        }
    }

    /// Returns the raw pointer to the MDBX transaction.
    ///
    /// # Safety
    ///
    /// The caller MUST NOT perform any mdbx operations on the returned pointer
    /// unless the caller ALSO holds the lock returned by [`Self::lock`].
    #[cfg(feature = "read-tx-timeouts")]
    pub(crate) const unsafe fn txn_ptr(&self) -> *mut ffi::MDBX_txn {
        self.txn
    }

    /// Acquires the inner transaction lock to guarantee exclusive access to the transaction
    /// pointer.
    pub(crate) fn lock(&self) -> MutexGuard<'_, bool> {
        if let Some(lock) = self.lock.try_lock() {
            lock
        } else {
            tracing::trace!(
                target: "libmdbx",
                txn = %self.txn as usize,
                backtrace = %std::backtrace::Backtrace::capture(),
                "Transaction lock is already acquired, blocking...
                To display the full backtrace, run with `RUST_BACKTRACE=full` env variable."
            );
            self.lock.lock()
        }
    }

    /// Executes the given closure once the lock on the transaction is acquired.
    ///
    /// Returns the result of the closure or an error if the transaction is
    /// timed out.
    #[inline]
    pub(crate) fn txn_execute_fail_on_timeout<F, T>(&self, f: F) -> MdbxResult<T>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        self.with_txn_ptr(f)
    }

    /// Executes the given closure once the lock on the transaction is
    /// acquired. If the transaction is timed out, it will be renewed first.
    ///
    /// Returns the result of the closure or an error if the transaction renewal fails.
    #[inline]
    pub(crate) fn txn_execute_renew_on_timeout<F, T>(&self, f: F) -> MdbxResult<T>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        let _lck = self.lock();

        // To be able to do any operations on the transaction, we need to renew it first.
        #[cfg(feature = "read-tx-timeouts")]
        if *_lck {
            use crate::error::mdbx_result;
            mdbx_result(unsafe { ffi::mdbx_txn_renew(self.txn) })?;
        }

        Ok((f)(self.txn))
    }

    /// Returns a reference to the environment that owns this transaction.
    pub(crate) const fn env(&self) -> &Environment {
        &self.env
    }

    /// Returns the tracing span for this transaction.
    pub(crate) const fn span(&self) -> &tracing::Span {
        &self.span
    }
}

impl<K: TransactionKind> TxPtrAccess for PtrSyncInner<K> {
    fn with_txn_ptr<F, R>(&self, f: F) -> MdbxResult<R>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> R,
    {
        let timeout_flag = self.lock();
        if *timeout_flag {
            return Err(crate::MdbxError::ReadTransactionTimeout);
        }
        let result = f(self.txn);
        Ok(result)
    }

    fn with_txn_ptr_for_cleanup<F, R>(&self, f: F) -> MdbxResult<R>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> R,
    {
        self.txn_execute_renew_on_timeout(f)
    }

    fn mark_committed(&self) {
        self.committed.store(true, Ordering::SeqCst);
    }
}

impl<K: TransactionKind> Drop for PtrSyncInner<K> {
    fn drop(&mut self) {
        if self.committed.load(Ordering::SeqCst) {
            return;
        }

        let _guard = self.span().enter();
        tracing::debug!(target: "libmdbx", "aborted");

        // RO transactions can be aborted directly.
        if K::IS_READ_ONLY {
            #[cfg(feature = "read-tx-timeouts")]
            self.env.txn_manager().remove_active_read_transaction(self.txn);

            unsafe {
                ffi::mdbx_txn_abort(self.txn);
            }
        } else {
            // RW transactions need to be aborted via the txn manager.
            let (sender, rx) = sync_channel(0);
            self.env
                .txn_manager()
                .send_message(TxnManagerMessage::Abort { tx: RawTxPtr(self.txn), sender });
            rx.recv().unwrap().unwrap();
        }
    }
}

// SAFETY: Access to the transaction pointer is synchronized by the internal Mutex.
unsafe impl<K: TransactionKind> Send for PtrSyncInner<K> {}

// SAFETY: Access to the transaction pointer is synchronized by the internal Mutex.
unsafe impl<K: TransactionKind> Sync for PtrSyncInner<K> {}

#[cfg(test)]
mod test {
    use crate::tx::RoGuard;

    // Compile-time check: RO is Send
    const fn _assert_ro_send() {
        const fn _assert_send<T: Send>() {}
        _assert_send::<RoGuard>();
    }
}
