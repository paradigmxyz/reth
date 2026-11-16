use super::{Recorder, SetRecorderError};
use std::{
    cell::UnsafeCell,
    sync::atomic::{AtomicUsize, Ordering},
};

/// The recorder is uninitialized.
const UNINITIALIZED: usize = 0;

/// The recorder is currently being initialized.
const INITIALIZING: usize = 1;

/// The recorder has been initialized successfully and can be read.
const INITIALIZED: usize = 2;

/// An specialized version of `OnceCell` for `Recorder`.
#[derive(Debug)]
pub struct RecorderOnceCell {
    recorder: UnsafeCell<Option<&'static dyn Recorder>>,
    state: AtomicUsize,
}

impl RecorderOnceCell {
    /// Creates an uninitialized `RecorderOnceCell`.
    pub const fn new() -> Self {
        Self { recorder: UnsafeCell::new(None), state: AtomicUsize::new(UNINITIALIZED) }
    }

    pub fn set<R>(&self, recorder: R) -> Result<(), SetRecorderError<R>>
    where
        R: Recorder + 'static,
    {
        // Try and transition the cell from `UNINITIALIZED` to `INITIALIZING`, which would give
        // us exclusive access to set the recorder.
        match self.state.compare_exchange(
            UNINITIALIZED,
            INITIALIZING,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(UNINITIALIZED) => {
                unsafe {
                    // SAFETY: Access is unique because we can only be here if we won the race
                    // to transition from `UNINITIALIZED` to `INITIALIZING` above.
                    self.recorder.get().write(Some(Box::leak(Box::new(recorder))));
                }

                // Mark the recorder as initialized, which will make it visible to readers.
                self.state.store(INITIALIZED, Ordering::Release);
                Ok(())
            }
            _ => Err(SetRecorderError(recorder)),
        }
    }

    #[cfg(not(feature = "fuzz"))]
    pub fn try_load(&self) -> Option<&'static dyn Recorder> {
        if self.state.load(Ordering::Acquire) != INITIALIZED {
            None
        } else {
            // SAFETY: If the state is `INITIALIZED`, then we know that the recorder has been
            // installed and is safe to read.
            unsafe { self.recorder.get().read() }
        }
    }
}

// SAFETY: We can only mutate through `set`, which is protected by the `state` and unsafe
// function where the caller has to guarantee synced-ness.
unsafe impl Send for RecorderOnceCell {}
unsafe impl Sync for RecorderOnceCell {}
