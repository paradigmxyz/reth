use reth_node_builder::{
    components::PayloadServiceBuilder, BuilderContext, FullNodeTypes, PayloadTypes,
};
use reth_node_types::NodeTypesWithEngine;
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
use reth_provider::CanonStateSubscriptions;
use reth_scroll_engine_primitives::{ScrollBuiltPayload, ScrollPayloadBuilderAttributes};
use reth_scroll_payload::NoopPayloadJobGenerator;
use reth_scroll_primitives::ScrollPrimitives;
use reth_transaction_pool::TransactionPool;
use scroll_alloy_rpc_types_engine::ScrollPayloadAttributes;

/// Payload builder for Scroll.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScrollPayloadBuilder;

impl<Node, Pool> PayloadServiceBuilder<Node, Pool> for ScrollPayloadBuilder
where
    Node: FullNodeTypes,
    Node::Types: NodeTypesWithEngine<Primitives = ScrollPrimitives>,
    <Node::Types as NodeTypesWithEngine>::Engine: PayloadTypes<
        BuiltPayload = ScrollBuiltPayload,
        PayloadAttributes = ScrollPayloadAttributes,
        PayloadBuilderAttributes = ScrollPayloadBuilderAttributes,
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
        let payload_generator =
            NoopPayloadJobGenerator::<ScrollPayloadBuilderAttributes, ScrollBuiltPayload>::default(
            );
        let (payload_service, payload_builder) =
            PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

        ctx.task_executor().spawn_critical("payload builder service", Box::pin(payload_service));

        eyre::Ok(payload_builder)
    }
}
