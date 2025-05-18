//! Provides types and functions for stateless execution and validation of Ethereum blocks.
//!
//! This crate enables the verification of block execution without requiring access to a
//! full node's persistent database. Instead, it relies on pre-generated "witness" data
//! that proves the specific state accessed during the block's execution.
//!
//! # Key Components
//!
//! * `WitnessDatabase`: An implementation of [`reth_revm::Database`] that uses a
//!   [`reth_trie_sparse::SparseStateTrie`] populated from witness data, along with provided
//!   bytecode and ancestor block hashes, to serve state reads during execution.
//! * `stateless_validation`: The core function that orchestrates the stateless validation process.
//!   It takes a block, its execution witness, ancestor headers, and chain specification, then
//!   performs:
//!     1. Witness verification against the parent block's state root.
//!     2. Block execution using the `WitnessDatabase`.
//!     3. Post-execution consensus checks.
//!     4. Post-state root calculation and comparison against the block header.
//!
//! # Usage
//!
//! The primary entry point is typically the `validation::stateless_validation` function. Callers
//! need to provide the block to be validated along with accurately generated `ExecutionWitness`
//! data corresponding to that block's execution trace and the necessary Headers of ancestor
//! blocks.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![no_std]

extern crate alloc;

pub(crate) mod root;
/// Implementation of stateless validation
pub mod validation;
pub(crate) mod witness_db;

/// ForkSpec module
/// This is needed because ChainSpec is not serializable (neither is genesis)
///
/// When we parse execution spec tests, we get back a ForkSpec, that we then pass into
/// the guest program and convert it into a ChainSpec. If someone is using Hoodi/Mainnet
/// etc, then this may not be needed, as you can just do ChainSpec::mainnet() in the guest program
pub mod fork_spec;

#[doc(inline)]
pub use alloy_rpc_types_debug::ExecutionWitness;

use reth_ethereum_primitives::Block;
use reth_primitives_traits::RecoveredBlock;

/// ClientInput is a convenience structure for serializing the input needed
/// for the stateless validation function.
#[serde_with::serde_as]
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ClientInput {
    /// The block being executed in the stateless validation function
    #[serde_as(
        as = "reth_primitives_traits::serde_bincode_compat::RecoveredBlock<reth_ethereum_primitives::Block>"
    )]
    pub block: RecoveredBlock<Block>,
    /// ExecutionWitness for the stateless validation function
    pub witness: ExecutionWitness,
}

/// Tracks the amount of cycles a region of code takes up
/// in a zkvm environment and is no-op otherwise.
#[macro_export]
macro_rules! track_cycles {
    ($name:expr, $body:expr) => {{
        #[cfg(target_os = "zkvm")]
        {
            tracing::info!("cycle-tracker-report-start: {}", $name);
            let result = $body;
            tracing::info!("cycle-tracker-report-end: {}", $name);
            result
        }

        #[cfg(not(target_os = "zkvm"))]
        $body
    }};
}
