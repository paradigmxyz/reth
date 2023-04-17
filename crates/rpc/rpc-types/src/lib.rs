#![warn(missing_debug_implementations, missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth RPC type definitions
//!
//! Provides all relevant types for the various RPC endpoints, grouped by namespace.

mod admin;
mod eth;

pub use admin::*;
pub use eth::*;
use reth_primitives::{H256, U256};
use serde::{Deserialize, Serialize};

/// The status of the network being ran by the local node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkStatus {
    /// The local node client version.
    pub client_version: String,
    /// The current ethereum protocol version
    pub protocol_version: u64,
    /// Information about the Ethereum Wire Protocol.
    pub eth_protocol_info: EthProtocolInfo,
}
/// Information about the Ethereum Wire Protocol (ETH)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EthProtocolInfo {
    /// The current difficulty at the head of the chain.
    #[cfg_attr(
        feature = "serde",
        serde(deserialize_with = "reth_primitives::serde_helper::deserialize_json_u256")
    )]
    pub difficulty: U256,
    /// The block hash of the head of the chain.
    pub head: H256,
    /// Network ID in base 10.
    pub network: u64,
    /// Genesis block of the current chain.
    pub genesis: H256,
}
