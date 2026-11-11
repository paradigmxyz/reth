//! Scheduler-managed synchronization primitives.

use crate::executor::{
    deterministic::Timer,
    time::{Duration, Instant},
};
use std::sync::atomic::{AtomicBool, Ordering};

/// A one-time initialized value whose waiting callers remain scheduler-managed.
#[derive(Debug)]
pub struct OnceLock<T> {
    value: std::sync::OnceLock<T>,
    initializing: shuttle::sync::Mutex<()>,
    ready: shuttle::sync::Condvar,
}

impl<T> OnceLock<T> {
    /// Creates an uninitialized cell.
    pub const fn new() -> Self {
        Self {
            value: std::sync::OnceLock::new(),
            initializing: shuttle::sync::Mutex::new(()),
            ready: shuttle::sync::Condvar::new(),
        }
    }
    /// Returns the value if initialization has completed without blocking.
    pub fn get(&self) -> Option<&T> {
        self.value.get()
    }
    /// Blocks the logical thread until initialization completes.
    pub fn wait(&self) -> &T {
        let mut guard = self.initializing.lock().unwrap_or_else(|error| error.into_inner());
        while self.value.get().is_none() {
            guard = self.ready.wait(guard).unwrap_or_else(|error| error.into_inner());
        }
        drop(guard);
        self.value.get().expect("initialized cell")
    }
    /// Initializes the cell once, or returns `value` when already initialized.
    pub fn set(&self, value: T) -> Result<(), T> {
        let guard = self.initializing.lock().unwrap_or_else(|error| error.into_inner());
        let result = self.value.set(value);
        self.ready.notify_all();
        drop(guard);
        result
    }
    /// Returns the value, initializing it with `initialize` if necessary.
    ///
    /// An initializer panic leaves the cell uninitialized so a later caller can retry.
    pub fn get_or_init(&self, initialize: impl FnOnce() -> T) -> &T {
        if let Some(value) = self.value.get() {
            return value;
        }
        let guard = self.initializing.lock().unwrap_or_else(|error| error.into_inner());
        let value = self.value.get_or_init(initialize);
        self.ready.notify_all();
        drop(guard);
        value
    }
    /// Returns a mutable value when initialized, without blocking.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.value.get_mut()
    }
    /// Removes the initialized value, leaving the cell uninitialized.
    pub fn take(&mut self) -> Option<T> {
        self.value.take()
    }
    /// Consumes the cell and returns its initialized value.
    pub fn into_inner(self) -> Option<T> {
        self.value.into_inner()
    }
}
impl<T> Default for OnceLock<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> From<T> for OnceLock<T> {
    fn from(value: T) -> Self {
        Self {
            value: std::sync::OnceLock::from(value),
            initializing: shuttle::sync::Mutex::new(()),
            ready: shuttle::sync::Condvar::new(),
        }
    }
}

/// Non-poisoning locks with parking_lot-compatible locking methods.
pub mod parking_lot {
    pub use shuttle::sync::{MutexGuard, RwLockReadGuard, RwLockWriteGuard};

    /// A non-poisoning mutex managed by the simulation scheduler.
    #[derive(Debug)]
    pub struct Mutex<T>(shuttle::sync::Mutex<T>);
    impl<T> Mutex<T> {
        /// Creates a mutex containing `value`.
        pub const fn new(value: T) -> Self {
            Self(shuttle::sync::Mutex::new(value))
        }
        /// Acquires the mutex, blocking only the current logical thread.
        pub fn lock(&self) -> MutexGuard<'_, T> {
            self.0.lock().unwrap_or_else(|error| error.into_inner())
        }
        /// Attempts to acquire the mutex without waiting.
        pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
            match self.0.try_lock() {
                Ok(guard) => Some(guard),
                Err(shuttle::sync::TryLockError::Poisoned(error)) => Some(error.into_inner()),
                Err(shuttle::sync::TryLockError::WouldBlock) => None,
            }
        }
        /// Consumes the mutex, returning its value.
        pub fn into_inner(self) -> T {
            self.0.into_inner().unwrap_or_else(|error| error.into_inner())
        }
        /// Returns exclusive access without locking.
        pub fn get_mut(&mut self) -> &mut T {
            self.0.get_mut().unwrap_or_else(|error| error.into_inner())
        }
    }
    impl<T: Default> Default for Mutex<T> {
        fn default() -> Self {
            Self::new(T::default())
        }
    }
    impl<T> From<T> for Mutex<T> {
        fn from(value: T) -> Self {
            Self::new(value)
        }
    }

    /// A non-poisoning reader-writer lock managed by the simulation scheduler.
    #[derive(Debug)]
    pub struct RwLock<T>(shuttle::sync::RwLock<T>);
    impl<T> RwLock<T> {
        /// Creates a reader-writer lock containing `value`.
        pub const fn new(value: T) -> Self {
            Self(shuttle::sync::RwLock::new(value))
        }
        /// Acquires shared access, blocking only the current logical thread.
        pub fn read(&self) -> RwLockReadGuard<'_, T> {
            self.0.read().unwrap_or_else(|error| error.into_inner())
        }
        /// Acquires exclusive access, blocking only the current logical thread.
        pub fn write(&self) -> RwLockWriteGuard<'_, T> {
            self.0.write().unwrap_or_else(|error| error.into_inner())
        }
        /// Consumes the lock, returning its value.
        pub fn into_inner(self) -> T {
            self.0.into_inner().unwrap_or_else(|error| error.into_inner())
        }
        /// Returns exclusive access without locking.
        pub fn get_mut(&mut self) -> &mut T {
            self.0.get_mut().unwrap_or_else(|error| error.into_inner())
        }
    }
    impl<T: Default> Default for RwLock<T> {
        fn default() -> Self {
            Self::new(T::default())
        }
    }
    impl<T> From<T> for RwLock<T> {
        fn from(value: T) -> Self {
            Self::new(value)
        }
    }
}

/// A condition variable whose timeout waits use the virtual clock.
#[derive(Debug)]
pub struct Condvar(shuttle::sync::Condvar);

/// Whether a virtual condition-variable wait reached its deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitTimeoutResult(bool);
impl WaitTimeoutResult {
    /// Returns `true` when the timeout notification fired.
    pub const fn timed_out(&self) -> bool {
        self.0
    }
}

impl Condvar {
    /// Creates a condition variable.
    pub const fn new() -> Self {
        Self(shuttle::sync::Condvar::new())
    }
    /// Wakes one waiting logical thread.
    pub fn notify_one(&self) {
        self.0.notify_one();
    }
    /// Wakes every waiting logical thread.
    pub fn notify_all(&self) {
        self.0.notify_all();
    }
    /// Releases the mutex and blocks until notified, then reacquires it.
    pub fn wait<'a, T>(
        &self,
        guard: shuttle::sync::MutexGuard<'a, T>,
    ) -> shuttle::sync::LockResult<shuttle::sync::MutexGuard<'a, T>> {
        self.0.wait(guard)
    }
    /// Waits for a notification or a virtual timeout, then reacquires the mutex.
    ///
    /// Other waiters may wake spuriously when one waiter's timeout fires.
    pub fn wait_timeout<'a, T>(
        &self,
        guard: shuttle::sync::MutexGuard<'a, T>,
        duration: Duration,
    ) -> shuttle::sync::LockResult<(shuttle::sync::MutexGuard<'a, T>, WaitTimeoutResult)> {
        if duration.is_zero() {
            return Ok((guard, WaitTimeoutResult(true)));
        }
        let deadline = Instant::now() + duration;
        let timed_out = AtomicBool::new(false);
        let cancelled = AtomicBool::new(false);
        shuttle::thread::scope(|scope| {
            let timer = scope.spawn(|| {
                let timer = Timer::new(deadline);
                // Check cancellation before parking: an early unpark must not be lost.
                while !cancelled.load(Ordering::Acquire) && Instant::now() < deadline {
                    shuttle::thread::park();
                }
                drop(timer);
                if !cancelled.load(Ordering::Acquire) {
                    timed_out.store(true, Ordering::Release);
                    self.0.notify_all();
                }
            });
            let result = self.0.wait(guard);
            cancelled.store(true, Ordering::Release);
            timer.thread().unpark();
            timer.join().expect("condition-variable timeout notifier panicked");
            let timeout = WaitTimeoutResult(timed_out.load(Ordering::Acquire));
            result
                .map(|guard| (guard, timeout))
                .map_err(|error| shuttle::sync::PoisonError::new((error.into_inner(), timeout)))
        })
    }
    /// Waits until `condition` becomes false or the total virtual timeout expires.
    pub fn wait_timeout_while<'a, T, F>(
        &self,
        mut guard: shuttle::sync::MutexGuard<'a, T>,
        duration: Duration,
        mut condition: F,
    ) -> shuttle::sync::LockResult<(shuttle::sync::MutexGuard<'a, T>, WaitTimeoutResult)>
    where
        F: FnMut(&mut T) -> bool,
    {
        let deadline = Instant::now() + duration;
        while condition(&mut guard) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (next, timeout) = self.wait_timeout(guard, remaining)?;
            guard = next;
            if timeout.timed_out() {
                return Ok((guard, timeout));
            }
        }
        Ok((guard, WaitTimeoutResult(false)))
    }
}
impl Default for Condvar {
    fn default() -> Self {
        Self::new()
    }
}
