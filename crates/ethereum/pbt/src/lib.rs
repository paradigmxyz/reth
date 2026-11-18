//! Ethereum binary-tree fork support.
//!
//! Activation is selected by the Ethereum chain schedule (`binaryTrieTime`).
#![cfg(not(target_family = "wasm"))]

pub use reth_node_ethereum::pbt::*;

#[cfg(feature = "cli")]
pub mod cli;
