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
use clap::Parser;
use futures_util::stream::StreamExt;
use mev_share_sse::{client::EventStream, EventClient};
use reth::{
    cli::{
        components::RethNodeComponents,
        ext::{RethCliExt, RethNodeCommandConfig},
        Cli,
    },
    rpc::types::engine::beacon_api::events::{HeadEvent, PayloadAttributesEvent},
    tasks::TaskSpawner,
};
use std::net::{IpAddr, Ipv4Addr};
use tracing::{info, warn};

fn main() {
    Cli::<BeaconEventsExt>::parse().run().unwrap();
}

/// The type that tells the reth CLI what extensions to use
#[derive(Debug, Default)]
#[non_exhaustive]
struct BeaconEventsExt;

impl RethCliExt for BeaconEventsExt {
    /// This tells the reth CLI to install additional CLI arguments
    type Node = BeaconEventsConfig;
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

    #[arg(long = "topic", value_enum)]
    pub event_topic: BeaconNodeEventTopic,
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum BeaconNodeEventTopic {
    PayloadAtrributes,
    Head,
}

impl BeaconNodeEventTopic {
    fn to_string_representation(&self) -> String {
        match self {
            BeaconNodeEventTopic::PayloadAtrributes => "payload_attributes",
            BeaconNodeEventTopic::Head => "head",
        }
        .to_string()
    }
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
        let mut subscription = self.subscribe(&client).await;

        match self.event_topic {
            BeaconNodeEventTopic::PayloadAtrributes => {
                while let Some(event) = subscription.next().await {
                    let payload_attributes_event =
                        serde_json::from_str::<PayloadAttributesEvent>(&event.unwrap()).unwrap();
                    info!("Received payload attributes: {:?}", payload_attributes_event);
                }
            }
            BeaconNodeEventTopic::Head => {
                while let Some(event) = subscription.next().await {
                    let head_event = serde_json::from_str::<HeadEvent>(&event.unwrap()).unwrap();
                    info!("Received head event: {:?}", head_event);
                }
            }
        };

        while let Some(event) = subscription.next().await {
            info!("Received payload attributes: {:?}", event);
        }
    }

    // It can take a bit until the CL endpoint is live so we retry a few times
    async fn subscribe(&self, client: &EventClient) -> EventStream<String> {
        let beacon_node_events_url =
            format!("{}?topics={}", self.events_url(), self.event_topic.to_string_representation());
        loop {
            match client.subscribe(&beacon_node_events_url).await {
                Ok(subscription) => return subscription,
                Err(err) => {
                    warn!(
                        "Failed to subscribe to {:?} events: {:?}\nRetrying in 5 seconds...",
                        self.event_topic, err
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }
}

impl RethNodeCommandConfig for BeaconEventsConfig {
    fn on_node_started<Reth: RethNodeComponents>(&mut self, components: &Reth) -> eyre::Result<()> {
        components.task_executor().spawn(Box::pin(self.clone().run()));
        Ok(())
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
