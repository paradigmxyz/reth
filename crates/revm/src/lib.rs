//! Revm utils and implementations specific to reth.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub, rustdoc::all)]
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Contains glue code for integrating reth database into revm's [Database].
pub mod database;

/// revm implementation of reth block and transaction executors.
mod factory;

/// new revm account state executor
pub mod processor;

/// State changes that are not related to transactions.
pub mod state_change;

/// revm executor factory.
pub use factory::Factory;

/// reexport for convenience
pub use reth_revm_inspectors::*;
/// reexport for convenience
pub use reth_revm_primitives::*;

/// Re-export everything
pub use revm::{self, *};

/// Ethereum DAO hardfork state change data.
pub mod eth_dao_fork;
