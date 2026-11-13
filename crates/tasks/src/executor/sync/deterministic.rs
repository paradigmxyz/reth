//! Scheduler-managed synchronization primitives.

use shuttle_sync::sync;

/// A one-time initialized value whose waiting callers remain scheduler-managed.
#[derive(Debug)]
pub struct OnceLock<T> {
    value: std::sync::OnceLock<T>,
    initializing: sync::Mutex<()>,
    ready: sync::Condvar,
}

impl<T> OnceLock<T> {
    /// Creates an uninitialized cell.
    pub const fn new() -> Self {
        Self {
            value: std::sync::OnceLock::new(),
            initializing: sync::Mutex::new(()),
            ready: sync::Condvar::new(),
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
            initializing: sync::Mutex::new(()),
            ready: sync::Condvar::new(),
        }
    }
}
