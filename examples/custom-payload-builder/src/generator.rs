use crate::job::EmptyBlockPayloadJob;
use reth::{
    providers::{BlockReaderIdExt, StateProviderFactory},
    tasks::TaskSpawner,
    transaction_pool::TransactionPool,
};
use reth_basic_payload_builder::{BasicPayloadJobGeneratorConfig, PayloadBuilder, PayloadConfig};
use reth_payload_builder::{error::PayloadBuilderError, PayloadJobGenerator};
use reth_primitives::{Bytes, ChainSpec, SealedBlock};
use std::sync::Arc;

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
            _pool: self.pool.clone(),
            _executor: self.executor.clone(),
            _builder: self.builder.clone(),
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
    /// Creates a new [EmptyBlockPayloadJobGenerator] with the given config and custom
    /// [PayloadBuilder]
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
