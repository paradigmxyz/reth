#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! This crate contains a collection of traits and trait implementations for common database
//! operations.

/// Various provider traits.
mod traits;
pub use traits::{
    AccountProvider, BlockHashProvider, BlockProvider, HeaderProvider, StateProvider,
    StateProviderFactory,
};

/// Provider trait implementations.
pub mod providers;
pub use providers::{
    HistoricalStateProvider, HistoricalStateProviderRef, LatestStateProvider,
    LatestStateProviderRef, ShareableDatabase,
};

/// Common database utilities.
mod utils;
pub use utils::{insert_block, insert_canonical_block};

#[cfg(any(test, feature = "test-utils"))]
/// Common test helpers for mocking the Provider.
pub mod test_utils;

/// Re-export provider error.
pub use reth_interfaces::provider::Error;
