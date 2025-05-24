//! Example for how hook into the node via the CLI extension mechanism without registering
//! additional arguments
//!
//! Run with
//!
//! ```sh
//! cargo run -p custom-payload-builder -- node
//! ```
//!
//! This launches a regular reth node overriding the engine api payload builder with our custom.

#![warn(unused_crate_dependencies)]

use crate::generator::EmptyBlockPayloadJobGenerator;
use reth_basic_payload_builder::BasicPayloadJobGeneratorConfig;
use reth_ethereum::{
    chainspec::ChainSpec,
    cli::interface::Cli,
    node::{
        api::{node::FullNodeTypes, NodeTypes},
        builder::{components::PayloadServiceBuilder, BuilderContext},
        core::cli::config::PayloadBuilderConfig,
        node::EthereumAddOns,
        EthEngineTypes, EthEvmConfig, EthereumNode,
    },
    pool::{PoolTransaction, TransactionPool},
    provider::CanonStateSubscriptions,
    EthPrimitives, TransactionSigned,
};
use reth_ethereum_payload_builder::EthereumBuilderConfig;
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};

pub mod generator;
pub mod job;

#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct CustomPayloadBuilder;

impl<Node, Pool> PayloadServiceBuilder<Node, Pool, EthEvmConfig> for CustomPayloadBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypes<
            Payload = EthEngineTypes,
            ChainSpec = ChainSpec,
            Primitives = EthPrimitives,
        >,
    >,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>
        + Unpin
        + 'static,
{
    async fn spawn_payload_builder_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: EthEvmConfig,
    ) -> eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>> {
        tracing::info!("Spawning a custom payload builder");

        let payload_builder = reth_ethereum_payload_builder::EthereumPayloadBuilder::new(
            ctx.provider().clone(),
            pool,
            evm_config,
            EthereumBuilderConfig::new(),
        );

        let conf = ctx.payload_builder_config();

        let payload_job_config = BasicPayloadJobGeneratorConfig::default()
            .interval(conf.interval())
            .deadline(conf.deadline())
            .max_payload_tasks(conf.max_payload_tasks());

        let payload_generator = EmptyBlockPayloadJobGenerator::with_builder(
            ctx.provider().clone(),
            ctx.task_executor().clone(),
            payload_job_config,
            payload_builder,
        );

        let (payload_service, payload_builder) =
            PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

        ctx.task_executor()
            .spawn_critical("custom payload builder service", Box::pin(payload_service));

        Ok(payload_builder)
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
