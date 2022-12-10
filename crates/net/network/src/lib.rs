#![warn(missing_docs)]
#![deny(unused_must_use, rust_2018_idioms, rustdoc::broken_intra_doc_links)]
#![allow(rustdoc::private_intra_doc_links)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! reth P2P networking.
//!
//! Ethereum's networking protocol is specified in [devp2p](https://github.com/ethereum/devp2p).
//!
//! In order for a node to join the ethereum p2p network it needs to know what nodes are already
//! port of that network. This includes public identities (public key) and addresses (where to reach
//! them).
//!
//! ## Usage
//!
//! ### Configure and launch the network
//!
//! The [`NetworkConfig`] is used to configure the network.
//! It requires an instance of [`BlockProvider`](reth_provider::BlockProvider).
//!
//!
//! ```
//! # async fn launch() {
//! use std::sync::Arc;
//! use reth_network::config::{rng_secret_key, mainnet_nodes};
//! use reth_network::{NetworkConfig, NetworkManager};
//! use reth_provider::test_utils::TestApi;
//!
//! // This block provider implementation is used for testing purposes.
//! let client = Arc::new(TestApi::default());
//!
//! // The key that's used for encrypting sessions and to identify our node.
//! let local_key = rng_secret_key();
//!
//! let config = NetworkConfig::builder(client, local_key).boot_nodes(
//!     mainnet_nodes()
//! ).build();
//!
//! // create the network instance
//! let network = NetworkManager::new(config).await.unwrap();
//!
//! // keep a handle to the network and spawn it
//! let handle = network.handle().clone();
//! tokio::task::spawn(network);
//!
//! # }
//! ```

mod cache;
pub mod config;
mod discovery;
pub mod error;
pub mod eth_requests;
mod fetch;
mod import;
mod listener;
mod manager;
mod message;
mod network;
pub mod peers;
mod session;
mod state;
mod swarm;
pub mod transactions;

pub use config::NetworkConfig;
pub use fetch::FetchClient;
pub use manager::{NetworkEvent, NetworkManager};
pub use network::NetworkHandle;
pub use peers::PeersConfig;
