//! Synchronization primitives belonging to the selected backend.

pub use shuttle_sync::sync::*;

#[cfg(feature = "fuzz")]
mod deterministic;
#[cfg(feature = "fuzz")]
pub use deterministic::{Condvar, OnceLock, WaitTimeoutResult};

/// Non-poisoning locks selected by the official Shuttle wrapper.
pub mod parking_lot {
    pub use shuttle_parking_lot::*;
}
