use crate::job::EmptyBlockPayloadJob;
use alloy_eips::BlockNumberOrTag;
use reth::{
    api::Block,
    providers::{BlockReaderIdExt, BlockSource, StateProviderFactory},
    tasks::TaskSpawner,
};
use reth_basic_payload_builder::{
    BasicPayloadJobGeneratorConfig, HeaderForPayload, PayloadBuilder, PayloadConfig,
};
use reth_node_api::PayloadBuilderAttributes;
use reth_payload_builder::{PayloadBuilderError, PayloadJobGenerator};
use reth_primitives::SealedHeader;
use std::sync::Arc;

/// The generator type that creates new jobs that builds empty blocks.
#[derive(Debug)]
pub struct EmptyBlockPayloadJobGenerator<Client, Tasks, Builder> {
    /// The client that can interact with the chain.
    client: Client,
    /// How to spawn building tasks
    executor: Tasks,
    /// The configuration for the job generator.
    _config: BasicPayloadJobGeneratorConfig,
    /// The type responsible for building payloads.
    ///
    /// See [PayloadBuilder]
    builder: Builder,
}

// === impl EmptyBlockPayloadJobGenerator ===

impl<Client, Tasks, Builder> EmptyBlockPayloadJobGenerator<Client, Tasks, Builder> {
    /// Creates a new [EmptyBlockPayloadJobGenerator] with the given config and custom
    /// [PayloadBuilder]
    pub fn with_builder(
        client: Client,
        executor: Tasks,
        config: BasicPayloadJobGeneratorConfig,
        builder: Builder,
    ) -> Self {
        Self { client, executor, _config: config, builder }
    }
}

impl<Client, Tasks, Builder> PayloadJobGenerator
    for EmptyBlockPayloadJobGenerator<Client, Tasks, Builder>
where
    Client: StateProviderFactory
        + BlockReaderIdExt<Header = HeaderForPayload<Builder::BuiltPayload>>
        + Clone
        + Unpin
        + 'static,
    Tasks: TaskSpawner + Clone + Unpin + 'static,
    Builder: PayloadBuilder + Unpin + 'static,
    Builder::Attributes: Unpin + Clone,
    Builder::BuiltPayload: Unpin + Clone,
{
    type Job = EmptyBlockPayloadJob<Tasks, Builder>;

    /// This is invoked when the node receives payload attributes from the beacon node via
    /// `engine_forkchoiceUpdatedV1`
    fn new_payload_job(
        &self,
        attributes: Builder::Attributes,
    ) -> Result<Self::Job, PayloadBuilderError> {
        let parent_block = if attributes.parent().is_zero() {
            // use latest block if parent is zero: genesis block
            self.client
                .block_by_number_or_tag(BlockNumberOrTag::Latest)?
                .ok_or_else(|| PayloadBuilderError::MissingParentBlock(attributes.parent()))?
                .seal_slow()
        } else {
            let block = self
                .client
                .find_block_by_hash(attributes.parent(), BlockSource::Any)?
                .ok_or_else(|| PayloadBuilderError::MissingParentBlock(attributes.parent()))?;

            // we already know the hash, so we can seal it
            block.seal_unchecked(attributes.parent())
        };
        let hash = parent_block.hash();
        let header = SealedHeader::new(parent_block.header().clone(), hash);

        let config = PayloadConfig::new(Arc::new(header), attributes);
        Ok(EmptyBlockPayloadJob {
            _executor: self.executor.clone(),
            builder: self.builder.clone(),
            config,
        })
    }
}
