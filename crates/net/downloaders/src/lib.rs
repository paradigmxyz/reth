#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
#![allow(clippy::result_large_err)]

//! Implements the downloader algorithms.

/// The collection of algorithms for downloading block bodies.
pub mod bodies;

/// The collection of algorithms for downloading block headers.
pub mod headers;

/// Common downloader metrics.
pub mod metrics;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
