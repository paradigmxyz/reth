use crate::job::EmptyBlockPayloadJob;
use alloy_eips::BlockNumberOrTag;
use reth_basic_payload_builder::{
    BasicPayloadJobGeneratorConfig, HeaderForPayload, PayloadBuilder, PayloadConfig,
};
use reth_ethereum::{
    node::api::Block,
    primitives::SealedHeader,
    provider::{BlockReaderIdExt, BlockSource, StateProviderFactory},
    tasks::Runtime,
};
use reth_payload_builder::{BuildNewPayload, PayloadBuilderError, PayloadId, PayloadJobGenerator};
use std::sync::Arc;

/// The generator type that creates new jobs that builds empty blocks.
#[derive(Debug)]
pub struct EmptyBlockPayloadJobGenerator<Client, Builder> {
    /// The client that can interact with the chain.
    client: Client,
    /// How to spawn building tasks
    executor: Runtime,
    /// The configuration for the job generator.
    _config: BasicPayloadJobGeneratorConfig,
    /// The type responsible for building payloads.
    ///
    /// See [PayloadBuilder]
    builder: Builder,
}

// === impl EmptyBlockPayloadJobGenerator ===

impl<Client, Builder> EmptyBlockPayloadJobGenerator<Client, Builder> {
    /// Creates a new [EmptyBlockPayloadJobGenerator] with the given config and custom
    /// [PayloadBuilder]
    pub fn with_builder(
        client: Client,
        executor: Runtime,
        config: BasicPayloadJobGeneratorConfig,
        builder: Builder,
    ) -> Self {
        Self { client, executor, _config: config, builder }
    }
}

impl<Client, Builder> PayloadJobGenerator for EmptyBlockPayloadJobGenerator<Client, Builder>
where
    Client: StateProviderFactory
        + BlockReaderIdExt<Header = HeaderForPayload<Builder::BuiltPayload>>
        + Clone
        + Unpin
        + 'static,
    Builder: PayloadBuilder + Unpin + 'static,
    Builder::Attributes: Unpin + Clone,
    Builder::BuiltPayload: Unpin + Clone,
{
    type Job = EmptyBlockPayloadJob<Builder>;

    /// This is invoked when the node receives payload attributes from the beacon node via
    /// `engine_forkchoiceUpdatedV1`
    fn new_payload_job(
        &self,
        input: BuildNewPayload<Builder::Attributes>,
        id: PayloadId,
    ) -> Result<Self::Job, PayloadBuilderError> {
        let parent_block = if input.parent_hash.is_zero() {
            // use latest block if parent is zero: genesis block
            self.client
                .block_by_number_or_tag(BlockNumberOrTag::Latest)?
                .ok_or_else(|| PayloadBuilderError::MissingParentBlock(input.parent_hash))?
                .seal_slow()
        } else {
            let block = self
                .client
                .find_block_by_hash(input.parent_hash, BlockSource::Any)?
                .ok_or_else(|| PayloadBuilderError::MissingParentBlock(input.parent_hash))?;

            // we already know the hash, so we can seal it
            block.seal_unchecked(input.parent_hash)
        };
        let hash = parent_block.hash();
        let header = SealedHeader::new(parent_block.header().clone(), hash);

        let config = PayloadConfig::new(Arc::new(header), input.attributes, id);
        Ok(EmptyBlockPayloadJob {
            _executor: self.executor.clone(),
            builder: self.builder.clone(),
            config,
        })
    }
}
