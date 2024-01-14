//! Revm utils and implementations specific to reth.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
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
pub use factory::EvmProcessorFactory;

/// reexport for convenience
pub use revm_inspectors::*;

/// Re-export everything
pub use revm::{self, *};

/// Ethereum DAO hardfork state change data.
pub mod eth_dao_fork;

/// Optimism-specific implementation and utilities for the executor
#[cfg(feature = "optimism")]
pub mod optimism;
