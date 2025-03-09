//! Example for how to instantiate a payload builder that emits events when a better payload is built.
//!
//! Run with
//!
//! ```sh
//! cargo run -p example-better-payload-events -- node
//! ```
//!
//! This launches a regular reth node overriding the engine api payload builder with a [`reth_basic_payload_builder::BetterPayloadEmitter`].

#![warn(unused_crate_dependencies)]

use builder::BetterPayloadEmitterBuilder;
use reth::{builder::components::BasicPayloadServiceBuilder, cli::Cli};
use reth_node_ethereum::{node::EthereumAddOns, EthereumNode};
use reth_payload_builder::EthBuiltPayload;
use tokio::sync::broadcast;

mod builder;

fn main() {
    let (tx, rx) = broadcast::channel::<EthBuiltPayload>(16);
    let payload_builder = BasicPayloadServiceBuilder::new(BetterPayloadEmitterBuilder::new(tx));

    Cli::parse_args()
        .run(|builder, _| async move {
            let handle = builder
                .with_types::<EthereumNode>()
                .with_components(EthereumNode::components().payload(payload_builder))
                .with_add_ons(EthereumAddOns::default())
                .launch()
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}
