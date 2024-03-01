//! Example of how to subscribe to beacon chain events via SSE.
//!
//! See also [ethereum-beacon-API eventstream](https://ethereum.github.io/beacon-APIs/#/Events/eventstream)
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p beacon-api-sse -- node
//! ```
//!
//! This launches a regular reth instance and subscribes to payload attributes event stream.
//!
//! **NOTE**: This expects that the CL client is running an http server on `localhost:5052` and is
//! configured to emit payload attributes events.
//!
//! See lighthouse beacon Node API: <https://lighthouse-book.sigmaprime.io/api-bn.html#beacon-node-api>

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use clap::Parser;
use futures_util::stream::StreamExt;
use mev_share_sse::{client::EventStream, EventClient};
use reth::{cli::Cli, rpc::types::beacon::events::PayloadAttributesEvent};
use reth_node_ethereum::EthereumNode;
use std::net::{IpAddr, Ipv4Addr};
use tracing::{info, warn};

fn main() {
    Cli::<BeaconEventsConfig>::parse()
        .run(|builder, args| async move {
            let handle = builder.node(EthereumNode::default()).launch().await?;

            handle.node.task_executor.spawn(Box::pin(args.run()));

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, clap::Parser)]
struct BeaconEventsConfig {
    /// Beacon Node http server address
    #[arg(long = "cl.addr", default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    pub cl_addr: IpAddr,
    /// Beacon Node http server port to listen on
    #[arg(long = "cl.port", default_value_t = 5052)]
    pub cl_port: u16,
}

impl BeaconEventsConfig {
    /// Returns the http url of the beacon node
    pub fn http_base_url(&self) -> String {
        format!("http://{}:{}", self.cl_addr, self.cl_port)
    }

    /// Returns the URL to the events endpoint
    pub fn events_url(&self) -> String {
        format!("{}/eth/v1/events", self.http_base_url())
    }

    /// Service that subscribes to beacon chain payload attributes events
    async fn run(self) {
        let client = EventClient::default();

        let mut subscription = self.new_payload_attributes_subscription(&client).await;

        while let Some(event) = subscription.next().await {
            info!("Received payload attributes: {:?}", event);
        }
    }

    // It can take a bit until the CL endpoint is live so we retry a few times
    async fn new_payload_attributes_subscription(
        &self,
        client: &EventClient,
    ) -> EventStream<PayloadAttributesEvent> {
        let payloads_url = format!("{}?topics=payload_attributes", self.events_url());
        loop {
            match client.subscribe(&payloads_url).await {
                Ok(subscription) => return subscription,
                Err(err) => {
                    warn!("Failed to subscribe to payload attributes events: {:?}\nRetrying in 5 seconds...", err);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_config() {
        let args = BeaconEventsConfig::try_parse_from(["reth"]);
        assert!(args.is_ok());
    }
}
