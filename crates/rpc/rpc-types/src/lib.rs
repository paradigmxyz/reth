//! Reth RPC type definitions.
//!
//! Provides all relevant types for the various RPC endpoints, grouped by namespace.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#[allow(hidden_glob_reexports)]
mod eth;

/// Alias for a peer identifier
pub type PeerId = B512;

use alloy_primitives::B512;
// re-export for convenience
pub use alloy_rpc_types::serde_helpers;

// Ethereum specific rpc types coming from alloy.
pub use alloy_rpc_types::*;

// Ethereum specific serde types coming from alloy.
pub use alloy_serde::*;

pub mod trace {
    //! RPC types for trace endpoints and inspectors.
    pub use alloy_rpc_types_trace::*;
}

// re-export admin
pub use alloy_rpc_types_admin as admin;

// Anvil specific rpc types coming from alloy.
pub use alloy_rpc_types_anvil as anvil;

// re-export mev
pub use alloy_rpc_types_mev as mev;

// re-export beacon
pub use alloy_rpc_types_beacon as beacon;

// re-export txpool
pub use alloy_rpc_types_txpool as txpool;

// Ethereum specific rpc types related to typed transaction requests and the engine API.
pub use eth::{
    engine,
    engine::{
        ExecutionPayload, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, PayloadError,
    },
    transaction::{self, TransactionRequest, TypedTransactionRequest},
};
#[cfg(feature = "default")]
pub use eth::error::ToRpcError;
