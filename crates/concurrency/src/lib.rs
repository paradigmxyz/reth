//! Concurrency primitives selected for native execution or opt-in fuzz simulation.
//!
//! The `fuzz` feature alone keeps native primitives. Add `--cfg dst` to `RUSTFLAGS`
//! with `fuzz` enabled to select simulation; callers must enter `sync::Runtime::run`.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

#[cfg(all(feature = "fuzz", dst))]
use parking_lot as _;
#[cfg(all(feature = "fuzz", dst))]
use rayon as _;
#[cfg(all(feature = "fuzz", dst))]
use reth_primitives_traits as _;

#[cfg(all(feature = "fuzz", not(dst)))]
use {either as _, shuttle as _, shuttle_parking_lot as _, shuttle_sync as _};

pub mod sync;

/// Marks a test that runs in the selected runtime. Requires the `std` feature.
#[cfg(feature = "std")]
pub use reth_concurrency_macros::test;
