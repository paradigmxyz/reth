#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! <reth crate template>

mod block;

pub mod db_provider;
mod state;

#[cfg(any(test, feature = "test-utils"))]
/// Common test helpers for mocking the Provider.
pub mod test_utils;

pub use block::{
    insert_canonical_block, BlockHashProvider, BlockProvider, ChainInfo, HeaderProvider,
};
pub use db_provider::{
    self as db, HistoricalStateProvider, HistoricalStateProviderRef, LatestStateProvider,
    LatestStateProviderRef, ProviderImpl,
};
pub use reth_interfaces::provider::Error;
pub use state::{AccountProvider, StateProvider, StateProviderFactory};
