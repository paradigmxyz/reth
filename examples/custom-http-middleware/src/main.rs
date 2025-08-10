//! This example shows how to configure custom components for a reth node.

#![warn(unused_crate_dependencies)]

use reth_node_builder::NodeHandle;
use reth_optimism_cli::{chainspec::OpChainSpecParser, Cli};
use reth_optimism_node::{args::RollupArgs, OpAddOns, OpNode};

use crate::middleware::CustomLayer;

pub mod middleware;

fn main() {
    if let Err(err) =
        Cli::<OpChainSpecParser, RollupArgs>::parse_args().run(|builder, args| async move {
            let node = OpNode::new(args);
            let NodeHandle { node: _node, node_exit_future } = builder
                .with_types::<OpNode>()
                .with_components(node.components())
                .with_add_ons(OpAddOns::default().with_tower_middleware(CustomLayer))
                .launch()
                .await?;

            node_exit_future.await?;
            Ok(())
        })
    {
        eprintln!("Error occurred: {}", err);
    }
}
