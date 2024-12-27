use reth_ethereum_engine_primitives::{
    EthBuiltPayload, EthPayloadAttributes, EthPayloadBuilderAttributes,
};
use reth_node_builder::{
    components::PayloadServiceBuilder, BuilderContext, FullNodeTypes, PayloadTypes,
};
use reth_node_types::NodeTypesWithEngine;
use reth_payload_builder::{
    test_utils::TestPayloadJobGenerator, PayloadBuilderHandle, PayloadBuilderService,
};
use reth_primitives::EthPrimitives;
use reth_provider::CanonStateSubscriptions;
use reth_transaction_pool::TransactionPool;

/// Payload builder for Scroll.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScrollPayloadBuilder;

impl<Node, Pool> PayloadServiceBuilder<Node, Pool> for ScrollPayloadBuilder
where
    Node: FullNodeTypes,
    Node::Types: NodeTypesWithEngine<Primitives = EthPrimitives>,
    <Node::Types as NodeTypesWithEngine>::Engine: PayloadTypes<
        BuiltPayload = EthBuiltPayload,
        PayloadAttributes = EthPayloadAttributes,
        PayloadBuilderAttributes = EthPayloadBuilderAttributes,
    >,
    Pool: TransactionPool,
{
    async fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        _pool: Pool,
    ) -> eyre::Result<
        PayloadBuilderHandle<<<Node as FullNodeTypes>::Types as NodeTypesWithEngine>::Engine>,
    > {
        let test_payload_generator = TestPayloadJobGenerator::default();
        let (payload_service, payload_builder) = PayloadBuilderService::new(
            test_payload_generator,
            ctx.provider().canonical_state_stream(),
        );

        ctx.task_executor().spawn_critical("payload builder service", Box::pin(payload_service));

        eyre::Ok(payload_builder)
    }
}
