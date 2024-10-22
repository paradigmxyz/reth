//! Run with
//!
//! ```not_rust
//! cargo run -p beacon-api-beacon-sidecar-fetcher --node -- full
//! ```
//!
//! This launches a regular reth instance and subscribes to payload attributes event stream.
//!
//! **NOTE**: This expects that the CL client is running an http server on `localhost:5052` and is
//! configured to emit payload attributes events.
//!
//! See beacon Node API: <https://ethereum.github.io/beacon-APIs/>

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use std::{
    collections::VecDeque,
    net::{IpAddr, Ipv4Addr},
};

use alloy_primitives::B256;
use clap::Parser;
use futures_util::{stream::FuturesUnordered, StreamExt};
use mined_sidecar::MinedSidecarStream;
use reth::{
    builder::NodeHandle, chainspec::EthereumChainSpecParser, cli::Cli,
    providers::CanonStateSubscriptions,
};
use reth_node_ethereum::EthereumNode;

pub mod mined_sidecar;

fn main() {
    Cli::<EthereumChainSpecParser, BeaconSidecarConfig>::parse()
        .run(|builder, beacon_config| async move {
            // launch the node
            let NodeHandle { node, node_exit_future } =
                builder.node(EthereumNode::default()).launch().await?;

            let notifications: reth::providers::CanonStateNotificationStream =
                node.provider.canonical_state_stream();

            let pool = node.pool.clone();

            node.task_executor.spawn(async move {
                let mut sidecar_stream = MinedSidecarStream {
                    events: notifications,
                    pool,
                    beacon_config,
                    client: reqwest::Client::new(),
                    pending_requests: FuturesUnordered::new(),
                    queued_actions: VecDeque::new(),
                };

                while let Some(result) = sidecar_stream.next().await {
                    match result {
                        Ok(blob_transaction) => {
                            // Handle successful transaction
                            println!("Processed BlobTransaction: {:?}", blob_transaction);
                        }
                        Err(e) => {
                            // Handle errors specifically
                            eprintln!("Failed to process transaction: {:?}", e);
                        }
                    }
                }
            });

            node_exit_future.await
        })
        .unwrap();
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, clap::Parser)]
pub struct BeaconSidecarConfig {
    /// Beacon Node http server address
    #[arg(long = "cl.addr", default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    pub cl_addr: IpAddr,
    /// Beacon Node http server port to listen on
    #[arg(long = "cl.port", default_value_t = 5052)]
    pub cl_port: u16,
}

impl Default for BeaconSidecarConfig {
    /// Default setup for lighthouse client
    fn default() -> Self {
        Self {
            cl_addr: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), // Equivalent to Ipv4Addr::LOCALHOST
            cl_port: 5052,
        }
    }
}

impl BeaconSidecarConfig {
    /// Returns the http url of the beacon node
    pub fn http_base_url(&self) -> String {
        format!("http://{}:{}", self.cl_addr, self.cl_port)
    }

    /// Returns the URL to the beacon sidecars endpoint <https://ethereum.github.io/beacon-APIs/#/Beacon/getBlobSidecars>
    pub fn sidecar_url(&self, block_root: B256) -> String {
        format!("{}/eth/v1/beacon/blob_sidecars/{}", self.http_base_url(), block_root)
    }
}
