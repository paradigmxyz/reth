//! This example showcases various Nodebuilder use cases

use reth_ethereum::{
    cli::interface::Cli,
    node::{builder::components::NoopNetworkBuilder, node::EthereumAddOns, EthereumNode},
};

/// Maps the ethereum node's network component to the noop implementation.
///
/// This installs the [`NoopNetworkBuilder`] that does not launch a real network.
pub fn noop_network() {
    Cli::parse_args()
        .run(|builder, _| async move {
            let handle = builder
                // use the default ethereum node types
                .with_types::<EthereumNode>()
                // Configure the components of the node
                // use default ethereum components but use the Noop network that does nothing but
                .with_components(EthereumNode::components().network(NoopNetworkBuilder::eth()))
                .with_add_ons(EthereumAddOns::default())
                .launch()
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

fn main() {}
