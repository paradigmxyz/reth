//! Synchronization primitives belonging to the selected backend.

#[cfg(not(feature = "fuzz"))]
pub use parking_lot;
#[cfg(feature = "fuzz")]
pub use shuttle::sync::*;
#[cfg(not(feature = "fuzz"))]
pub use std::sync::*;

#[cfg(feature = "fuzz")]
mod deterministic;
#[cfg(feature = "fuzz")]
pub use deterministic::{parking_lot, Condvar, OnceLock, WaitTimeoutResult};
