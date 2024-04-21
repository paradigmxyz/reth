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

use generator::EmptyBlockPayloadJobGenerator;
use reth::{
    builder::{components::PayloadServiceBuilder, node::FullNodeTypes, BuilderContext},
    cli::{config::PayloadBuilderConfig, Cli},
    payload::PayloadBuilderHandle,
    providers::CanonStateSubscriptions,
    transaction_pool::TransactionPool,
};
use reth_basic_payload_builder::BasicPayloadJobGeneratorConfig;
use reth_node_ethereum::{EthEngineTypes, EthereumNode};
use reth_payload_builder::PayloadBuilderService;

pub mod generator;
pub mod job;

#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct CustomPayloadBuilder;

impl<Node, Pool> PayloadServiceBuilder<Node, Pool> for CustomPayloadBuilder
where
    Node: FullNodeTypes<Engine = EthEngineTypes>,
    Pool: TransactionPool + Unpin + 'static,
{
    async fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<Node::Engine>> {
        tracing::info!("Spawning a custom payload builder");
        let conf = ctx.payload_builder_config();

        let payload_job_config = BasicPayloadJobGeneratorConfig::default()
            .interval(conf.interval())
            .deadline(conf.deadline())
            .max_payload_tasks(conf.max_payload_tasks())
            .extradata(conf.extradata_bytes())
            .max_gas_limit(conf.max_gas_limit());

        let payload_generator = EmptyBlockPayloadJobGenerator::with_builder(
            ctx.provider().clone(),
            pool,
            ctx.task_executor().clone(),
            payload_job_config,
            ctx.chain_spec().clone(),
            reth_ethereum_payload_builder::EthereumPayloadBuilder::default(),
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
                .with_types(EthereumNode::default())
                // Configure the components of the node
                // use default ethereum components but use our custom payload builder
                .with_components(
                    EthereumNode::components().payload(CustomPayloadBuilder::default()),
                )
                .launch()
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}
