//! Example for how hook into the node via the CLI extension mechanism without registering
//! additional arguments
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p node-event-hooks -- node
//! ```
//!
//! This launch the regular reth node and also print:
//!
//! > "All components initialized"
//! once all components have been initialized and
//!
//! > "Node started"
//! once the node has been started.

use reth::api::PayloadBuilderAttributes;
use reth::cli::Cli;
use reth::primitives::{Address, B256};
use reth::rpc::types::engine::PayloadAttributes;
use reth_node_ethereum::EthereumNode;

fn main() {
    Cli::parse_args()
        .run(|builder, _| async move {
            let handle = builder
                .node(EthereumNode::default())
                .on_node_started(|_ctx| {
                    println!("Node started");
                    Ok(())
                })
                .on_rpc_started(|_ctx, _handles| {
                    println!("RPC started");
                    Ok(())
                })
                .on_component_initialized(|_ctx| {
                    println!("All components initialized");
                    Ok(())
                })
                .launch()
                .await?;


            let rpc_payload_attributes = PayloadAttributes {
                timestamp: 10000,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: Address::default(),
                withdrawals: None,
                parent_beacon_block_root: None,
            };
            let attr = PayloadBuilderAttributes::try_new(B256::ZERO, rpc_payload_attributes).unwrap();
            let payload_builder = handle.node.payload_builder.clone();
            payload_builder.new_payload(attr);

            handle.wait_for_node_exit().await
        })
        .unwrap();
}
