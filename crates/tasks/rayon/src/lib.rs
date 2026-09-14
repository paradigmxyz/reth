//! Rayon operations used by Reth.
//!
//! The `dst` feature replaces native dispatch with inline execution while a deterministic task is
//! polled. Default builds compile only the direct Rayon implementation.

#[cfg(feature = "dst")]
mod dst;
#[cfg(feature = "dst")]
pub use dst::*;

#[cfg(not(feature = "dst"))]
mod native;
#[cfg(not(feature = "dst"))]
pub use native::*;
