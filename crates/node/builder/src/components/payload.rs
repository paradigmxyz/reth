//! Payload service component for the node builder.

use crate::{BuilderContext, FullNodeTypes};
use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_chain_state::CanonStateSubscriptions;
use reth_node_api::{NodeTypes, PayloadBuilderFor};
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
use reth_transaction_pool::TransactionPool;
use std::future::Future;

/// A type that knows how to spawn the payload service.
pub trait PayloadServiceBuilder<Node: FullNodeTypes, Pool: TransactionPool>: Send + Sized {
    /// Spawns the [`PayloadBuilderService`] and returns the handle to it for use by the engine.
    ///
    /// We provide default implementation via [`BasicPayloadJobGenerator`] but it can be overridden
    /// for custom job orchestration logic,
    fn spawn_payload_builder_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> impl Future<Output = eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>>>
           + Send;
}

impl<Node, F, Fut, Pool> PayloadServiceBuilder<Node, Pool> for F
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    F: Fn(&BuilderContext<Node>, Pool) -> Fut + Send,
    Fut: Future<Output = eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>>>
        + Send,
{
    fn spawn_payload_builder_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> impl Future<Output = eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>>>
    {
        self(ctx, pool)
    }
}

/// A type that knows how to build a payload builder to plug into [`BasicPayloadServiceBuilder`].
pub trait PayloadBuilderBuilder<Node: FullNodeTypes, Pool: TransactionPool>: Send + Sized {
    /// Payload builder implementation.
    type PayloadBuilder: PayloadBuilderFor<Node::Types> + Unpin + 'static;

    /// Spawns the payload service and returns the handle to it.
    ///
    /// The [`BuilderContext`] is provided to allow access to the node's configuration.
    fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> impl Future<Output = eyre::Result<Self::PayloadBuilder>> + Send;
}

/// Basic payload service builder that spawns a [`BasicPayloadJobGenerator`]
#[derive(Debug, Default, Clone)]
pub struct BasicPayloadServiceBuilder<PB>(PB);

impl<PB> BasicPayloadServiceBuilder<PB> {
    /// Create a new [`BasicPayloadServiceBuilder`].
    pub const fn new(payload_builder_builder: PB) -> Self {
        Self(payload_builder_builder)
    }
}

impl<Node, Pool, PB> PayloadServiceBuilder<Node, Pool> for BasicPayloadServiceBuilder<PB>
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    PB: PayloadBuilderBuilder<Node, Pool>,
{
    async fn spawn_payload_builder_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>> {
        let payload_builder = self.0.build_payload_builder(ctx, pool).await?;

        let conf = ctx.config().builder.clone();

        let payload_job_config = BasicPayloadJobGeneratorConfig::default()
            .interval(conf.interval)
            .deadline(conf.deadline)
            .max_payload_tasks(conf.max_payload_tasks);

        let payload_generator = BasicPayloadJobGenerator::with_builder(
            ctx.provider().clone(),
            ctx.task_executor().clone(),
            payload_job_config,
            payload_builder,
        );
        let (payload_service, payload_service_handle) =
            PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

        ctx.task_executor().spawn_critical("payload builder service", Box::pin(payload_service));

        Ok(payload_service_handle)
    }
}
