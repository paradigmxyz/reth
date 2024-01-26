//! use std::future::Future;
use futures_util::Future;
use reth::{
    providers::{BlockReaderIdExt, StateProviderFactory},
    tasks::TaskSpawner,
    transaction_pool::TransactionPool,
};
use reth_basic_payload_builder::{BasicPayloadJobGeneratorConfig, PayloadBuilder, PayloadConfig};
use reth_payload_builder::{
    error::PayloadBuilderError, KeepPayloadJobAlive, PayloadJob, PayloadJobGenerator,
};
use reth_primitives::{Bytes, ChainSpec, SealedBlock};

use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

impl<Client, Pool, Tasks, Builder> PayloadJobGenerator
    for EmptyBlockPayloadJobGenerator<Client, Pool, Tasks, Builder>
where
    Client: StateProviderFactory + BlockReaderIdExt + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + Unpin + 'static,
    Builder: PayloadBuilder<Pool, Client> + Unpin + 'static,
    <Builder as PayloadBuilder<Pool, Client>>::Attributes: Unpin + Clone,
    <Builder as PayloadBuilder<Pool, Client>>::BuiltPayload: Unpin + Clone,
{
    type Job = EmptyBlockPayloadJob<Client, Pool, Tasks, Builder>;

    /// This is invoked when the node receives payload attributes from the beacon node via
    /// `engine_forkchoiceUpdatedV1`
    fn new_payload_job(
        &self,
        attributes: <Builder as PayloadBuilder<Pool, Client>>::Attributes,
    ) -> Result<Self::Job, PayloadBuilderError> {
        let config = PayloadConfig::new(
            Arc::new(SealedBlock::default()),
            Bytes::default(),
            attributes,
            Arc::new(ChainSpec::default()),
        );
        Ok(EmptyBlockPayloadJob {
            client: self.client.clone(),
            pool: self.pool.clone(),
            executor: self.executor.clone(),
            builder: self.builder.clone(),
            config,
        })
    }
}

/// The generator type that creates new jobs that builds empty blocks.
#[derive(Debug)]
pub struct EmptyBlockPayloadJobGenerator<Client, Pool, Tasks, Builder> {
    /// The client that can interact with the chain.
    client: Client,
    /// txpool
    pool: Pool,
    /// How to spawn building tasks
    executor: Tasks,
    /// The configuration for the job generator.
    _config: BasicPayloadJobGeneratorConfig,
    /// The type responsible for building payloads.
    ///
    /// See [PayloadBuilder]
    builder: Builder,
}

// === impl BasicPayloadJobGenerator ===

impl<Client, Pool, Tasks, Builder> EmptyBlockPayloadJobGenerator<Client, Pool, Tasks, Builder> {
    /// Creates a new [BasicPayloadJobGenerator] with the given config and custom [PayloadBuilder]
    pub fn with_builder(
        client: Client,
        pool: Pool,
        executor: Tasks,
        config: BasicPayloadJobGeneratorConfig,
        builder: Builder,
    ) -> Self {
        Self { client, pool, executor, _config: config, builder }
    }
}

/// A [PayloadJob] that builds empty blocks.
pub struct EmptyBlockPayloadJob<Client, Pool, Tasks, Builder>
where
    Builder: PayloadBuilder<Pool, Client>,
{
    /// The configuration for how the payload will be created.
    config: PayloadConfig<Builder::Attributes>,
    /// The client that can interact with the chain.
    client: Client,
    /// The transaction pool.
    pool: Pool,
    /// How to spawn building tasks
    executor: Tasks,
    /// The type responsible for building payloads.
    ///
    /// See [PayloadBuilder]
    builder: Builder,
}

impl<Client, Pool, Tasks, Builder> PayloadJob for EmptyBlockPayloadJob<Client, Pool, Tasks, Builder>
where
    Client: StateProviderFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
    Builder: PayloadBuilder<Pool, Client> + Unpin + 'static,
    <Builder as PayloadBuilder<Pool, Client>>::Attributes: Unpin + Clone,
    <Builder as PayloadBuilder<Pool, Client>>::BuiltPayload: Unpin + Clone,
{
    type PayloadAttributes = Builder::Attributes;
    type ResolvePayloadFuture =
        futures_util::future::Ready<Result<Self::BuiltPayload, PayloadBuilderError>>;
    type BuiltPayload = Builder::BuiltPayload;

    fn best_payload(&self) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let payload = Builder::build_empty_payload(&self.client, self.config.clone())?;
        Ok(payload)
    }

    fn payload_attributes(&self) -> Result<Self::PayloadAttributes, PayloadBuilderError> {
        Ok(self.config.attributes.clone())
    }

    fn resolve(&mut self) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive) {
        let payload = self.best_payload();
        (futures_util::future::ready(payload), KeepPayloadJobAlive::No)
    }
}

/// A [PayloadJob] is a a future that's being polled by the `PayloadBuilderService`
impl<Client, Pool, Tasks, Builder> Future for EmptyBlockPayloadJob<Client, Pool, Tasks, Builder>
where
    Client: StateProviderFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
    Builder: PayloadBuilder<Pool, Client> + Unpin + 'static,
    <Builder as PayloadBuilder<Pool, Client>>::Attributes: Unpin + Clone,
    <Builder as PayloadBuilder<Pool, Client>>::BuiltPayload: Unpin + Clone,
{
    type Output = Result<(), PayloadBuilderError>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}
