//! Revm executor implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub, rustdoc::all)]
#![deny(unused_must_use, rust_2018_idioms)]

/// revm implementation of reth block and transaction executors.
mod factory;
pub use factory::EVMProcessorFactory;

/// Aggregated execution data.
mod execution_data;
pub use execution_data::ExecutionData;

/// new revm account state executor
pub mod processor;

/// State changes that are not related to transactions.
pub mod state_change;

/// Ethereum DAO hardfork state change data.
pub mod eth_dao_fork;
