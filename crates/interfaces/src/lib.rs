#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth interface bindings

/// Consensus traits.
pub mod consensus;

/// Database error
pub mod db;

/// Block Execution traits.
pub mod executor;

/// Possible errors when interacting with the chain.
mod error;
pub use error::{Error, Result};

/// Traits for subscribing to events.
pub mod events;

/// P2P traits.
pub mod p2p;

/// Provider error
pub mod provider;

/// Syncing related traits.
pub mod sync;

#[cfg(any(test, feature = "test-utils"))]
/// Common test helpers for mocking out Consensus, Downloaders and Header Clients.
pub mod test_utils;
