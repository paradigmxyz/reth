//! Example for how hook into the node via the CLI extension mechanism without registering
//! additional arguments
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p custom-payload-builder -- node
//! ```
//!
//! This launch the regular reth node overriding the engine api payload builder with our custom.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use reth::{
    builder::{components::PayloadServiceBuilder, node::FullNodeTypes, BuilderContext},
    cli::{config::PayloadBuilderConfig, Cli},
    transaction_pool::{PoolTransaction, TransactionPool},
};
use reth_chainspec::ChainSpec;
use reth_ethereum_payload_builder::{EthereumBuilderConfig, EthereumPayloadBuilder};
use reth_node_api::NodeTypesWithEngine;
use reth_node_ethereum::{node::EthereumAddOns, EthEngineTypes, EthEvmConfig, EthereumNode};
use reth_primitives::{EthPrimitives, TransactionSigned};

pub mod generator;
pub mod job;

#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct CustomPayloadBuilder;

impl<Node, Pool> PayloadServiceBuilder<Node, Pool> for CustomPayloadBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypesWithEngine<
            Engine = EthEngineTypes,
            ChainSpec = ChainSpec,
            Primitives = EthPrimitives,
        >,
    >,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>
        + Unpin
        + 'static,
{
    type PayloadBuilder = EthereumPayloadBuilder<Pool, Node::Provider, EthEvmConfig>;

    async fn build_payload_builder(
        &self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<Self::PayloadBuilder> {
        tracing::info!("Spawning a custom payload builder");
        let conf = ctx.payload_builder_config();

        Ok(reth_ethereum_payload_builder::EthereumPayloadBuilder::new(
            ctx.provider().clone(),
            pool,
            EthEvmConfig::new(ctx.chain_spec()),
            EthereumBuilderConfig::new(conf.extra_data_bytes()),
        ))
    }
}

fn main() {
    Cli::parse_args()
        .run(|builder, _| async move {
            let handle = builder
                .with_types::<EthereumNode>()
                // Configure the components of the node
                // use default ethereum components but use our custom payload builder
                .with_components(
                    EthereumNode::components().payload(CustomPayloadBuilder::default()),
                )
                .with_add_ons(EthereumAddOns::default())
                .launch()
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}
