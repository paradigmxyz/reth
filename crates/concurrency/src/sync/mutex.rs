//! Mutex guards retain their lock so simulated condition variables can reacquire it safely.

use super::{LockResult, PoisonError, TryLockError, TryLockResult};
use std::{
    fmt,
    ops::{Deref, DerefMut},
};

/// A simulated mutex with the same poisoning behavior as Shuttle's mutex.
///
/// Shuttle tasks share an OS thread's unwinding state. A different logical task's unwind
/// can therefore poison this lock; task-local panic poisoning is not supported upstream.
#[derive(Debug)]
pub struct Mutex<T: ?Sized>(shuttle_sync::sync::Mutex<T>);

/// A guard that releases its simulated mutex when dropped.
pub struct MutexGuard<'a, T: ?Sized> {
    inner: shuttle_sync::sync::MutexGuard<'a, T>,
    mutex: &'a Mutex<T>,
}

impl<T> Mutex<T> {
    /// Creates an unlocked mutex containing `value`.
    pub const fn new(value: T) -> Self {
        Self(shuttle_sync::sync::Mutex::new(value))
    }

    /// Consumes this mutex, returning its data or a poison error containing the data.
    pub fn into_inner(self) -> LockResult<T> {
        self.0.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Blocks until the mutex is acquired, returning its guard even on poison.
    pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        self.0.lock().map(|inner| MutexGuard { inner, mutex: self }).map_err(|error| {
            PoisonError::new(MutexGuard { inner: error.into_inner(), mutex: self })
        })
    }

    /// Acquires without blocking, returning `WouldBlock` if held or the guard on poison.
    pub fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
        self.0.try_lock().map(|inner| MutexGuard { inner, mutex: self }).map_err(
            |error| match error {
                TryLockError::WouldBlock => TryLockError::WouldBlock,
                TryLockError::Poisoned(error) => {
                    TryLockError::Poisoned(PoisonError::new(MutexGuard {
                        inner: error.into_inner(),
                        mutex: self,
                    }))
                }
            },
        )
    }

    /// Returns exclusive access to the data, or a poison error containing the reference.
    pub fn get_mut(&mut self) -> LockResult<&mut T> {
        self.0.get_mut()
    }

    /// Clears the mutex's poisoned state.
    pub fn clear_poison(&self) {
        self.0.clear_poison();
    }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<'a, T: ?Sized> MutexGuard<'a, T> {
    pub(super) fn unlock(self) -> &'a Mutex<T> {
        let Self { inner, mutex } = self;
        drop(inner);
        mutex
    }
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl<T: ?Sized + fmt::Display> fmt::Display for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}
