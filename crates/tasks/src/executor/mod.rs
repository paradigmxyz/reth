//! Blocking execution primitives selected at compile time.
//!
//! The default backend uses native threads, Crossbeam channels, and Rayon. With
//! `fuzz`, run callers inside [`crate::executor::Runtime::run`]: stackful Shuttle tasks execute on
//! one OS thread in fixed round-robin order. Only primitives from this crate may
//! block simulated tasks; native blocking or OS I/O is outside the simulation.

#[cfg(feature = "fuzz")]
pub use shuttle::thread_local;
#[cfg(not(feature = "fuzz"))]
pub use std::thread_local;

pub mod channel;
pub mod iter;
pub mod pool;
pub mod runtime;
pub mod sync;
pub mod thread;
pub mod time;

#[cfg(feature = "fuzz")]
mod deterministic;

pub use pool::{ThreadPool, ThreadPoolBuilder};

pub use runtime::Runtime;

/// Rayon-compatible compute operations selected with the executor backend.
pub mod rayon {
    pub use crate::executor::pool::*;
    /// Parallel iterator extension traits for the selected compute backend.
    pub mod prelude {
        #[cfg(feature = "fuzz")]
        pub use crate::executor::iter::*;
        #[cfg(not(feature = "fuzz"))]
        pub use ::rayon::prelude::*;
    }
    /// Slice sorting operations for the selected compute backend.
    pub mod slice {
        #[cfg(feature = "fuzz")]
        pub use crate::executor::iter::ParallelSliceMut;
        #[cfg(not(feature = "fuzz"))]
        pub use ::rayon::slice::*;
    }
    /// Parallel iterator types for the selected compute backend.
    pub mod iter {
        pub use crate::executor::iter::*;
    }
}
