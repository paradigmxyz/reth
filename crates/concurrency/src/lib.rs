//! Concurrency primitives selected for native execution or fuzz simulation.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

#[cfg(feature = "fuzz")]
use parking_lot as _;
#[cfg(feature = "fuzz")]
use rayon as _;
#[cfg(feature = "fuzz")]
use reth_primitives_traits as _;

pub mod sync;
