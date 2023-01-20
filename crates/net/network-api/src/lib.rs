#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth network interface definitions.
//!
//! Provides abstractions for the reth-network crate.

use async_trait::async_trait;
use reth_primitives::{NodeRecord, H256, U256};
use serde::{Deserialize, Serialize};
use std::{error::Error, net::SocketAddr};

/// Provides general purpose information about the network.
#[async_trait]
pub trait NetworkInfo: Send + Sync {
    /// Associated error type for the network implementation.
    type Error: Send + Sync + Error;

    /// Returns the [`SocketAddr`] that listens for incoming connections.
    fn local_addr(&self) -> SocketAddr;

    /// Returns the current status of the network being ran by the local node.
    async fn network_status(&self) -> Result<NetworkStatus, Self::Error>;
}

/// Provides general purpose information about Peers in the network.
pub trait PeersInfo: Send + Sync {
    /// Returns how many peers the network is currently connected to.
    ///
    /// Note: this should only include established connections and _not_ ongoing attempts.
    fn num_connected_peers(&self) -> usize;

    /// Returns the Ethereum Node Record of the node.
    fn local_node_record(&self) -> NodeRecord;
}

/// The status of the network being ran by the local node.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct NetworkStatus {
    /// The local node client name.
    pub client_name: String,
    /// Information about the Ethereum Wire Protocol.
    pub eth_protocol_info: EthProtocolInfo,
}

/// Information about the Ethereum Wire Protocol (ETH)
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct EthProtocolInfo {
    /// The current difficulty at the head of the chain.
    pub difficulty: U256,
    /// The block hash of the head of the chain.
    pub head: H256,
    /// Network ID in base 10.
    pub network: u64,
    /// Genesis block of the current chain.
    pub genesis: H256,
}
