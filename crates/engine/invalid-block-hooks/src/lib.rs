#![warn(clippy::iter_over_hash_type)]

//! Invalid block hook implementations.

mod witness;

pub use witness::InvalidBlockWitnessHook;
