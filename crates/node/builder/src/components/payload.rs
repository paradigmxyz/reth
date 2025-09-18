//! Payload service component for the node builder.

use crate::{BuilderContext, FullNodeTypes};
use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_chain_state::CanonStateSubscriptions;
use reth_node_api::{NodeTypes, PayloadBuilderFor};
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService, PayloadServiceCommand};
use reth_transaction_pool::TransactionPool;
use std::future::Future;
use tokio::sync::{broadcast, mpsc};
use tracing::warn;

/// A type that knows how to spawn the payload service.
pub trait PayloadServiceBuilder<Node: FullNodeTypes, Pool: TransactionPool, EvmConfig>:
    Send + Sized
{
    /// Spawns the [`PayloadBuilderService`] and returns the handle to it for use by the engine.
    ///
    /// We provide default implementation via [`BasicPayloadJobGenerator`] but it can be overridden
    /// for custom job orchestration logic,
    fn spawn_payload_builder_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: EvmConfig,
    ) -> impl Future<Output = eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>>>
           + Send;
}

impl<Node, F, Fut, Pool, EvmConfig> PayloadServiceBuilder<Node, Pool, EvmConfig> for F
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    F: Fn(&BuilderContext<Node>, Pool, EvmConfig) -> Fut + Send,
    Fut: Future<Output = eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>>>
        + Send,
{
    fn spawn_payload_builder_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: EvmConfig,
    ) -> impl Future<Output = eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>>>
    {
        self(ctx, pool, evm_config)
    }
}

/// A type that knows how to build a payload builder to plug into [`BasicPayloadServiceBuilder`].
pub trait PayloadBuilderBuilder<Node: FullNodeTypes, Pool: TransactionPool, EvmConfig>:
    Send + Sized
{
    /// Payload builder implementation.
    type PayloadBuilder: PayloadBuilderFor<Node::Types> + Unpin + 'static;

    /// Spawns the payload service and returns the handle to it.
    ///
    /// The [`BuilderContext`] is provided to allow access to the node's configuration.
    fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: EvmConfig,
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

impl<Node, Pool, PB, EvmConfig> PayloadServiceBuilder<Node, Pool, EvmConfig>
    for BasicPayloadServiceBuilder<PB>
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    EvmConfig: Send,
    PB: PayloadBuilderBuilder<Node, Pool, EvmConfig>,
{
    async fn spawn_payload_builder_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: EvmConfig,
    ) -> eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>> {
        let payload_builder = self.0.build_payload_builder(ctx, pool, evm_config).await?;

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

/// A `NoopPayloadServiceBuilder` useful for node implementations that are not implementing
/// validating/sequencing logic.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct NoopPayloadServiceBuilder;

impl<Node, Pool, Evm> PayloadServiceBuilder<Node, Pool, Evm> for NoopPayloadServiceBuilder
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    Evm: Send,
{
    async fn spawn_payload_builder_service(
        self,
        ctx: &BuilderContext<Node>,
        _pool: Pool,
        _evm_config: Evm,
    ) -> eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>> {
        let (tx, mut rx) = mpsc::unbounded_channel();

        ctx.task_executor().spawn_critical("payload builder", async move {
            #[allow(clippy::collection_is_never_read)]
            let mut subscriptions = Vec::new();

            while let Some(message) = rx.recv().await {
                match message {
                    PayloadServiceCommand::Subscribe(tx) => {
                        let (events_tx, events_rx) = broadcast::channel(100);
                        // Retain senders to make sure that channels are not getting closed
                        subscriptions.push(events_tx);
                        let _ = tx.send(events_rx);
                    }
                    message => warn!(?message, "Noop payload service received a message"),
                }
            }
        });

        Ok(PayloadBuilderHandle::new(tx))
    }
}
