use crate::{
    engine_api::EngineApiTestContext, network::NetworkTestContext, payload::PayloadTestContext,
    rpc::RpcTestContext, traits::PayloadEnvelopeExt,
};

use alloy_rpc_types::BlockNumberOrTag;
use eyre::Ok;
use futures_util::Future;
use reth::{
    api::{BuiltPayload, EngineTypes, FullNodeComponents, PayloadBuilderAttributes},
    builder::FullNode,
    providers::{BlockReader, BlockReaderIdExt, CanonStateSubscriptions, StageCheckpointReader},
    rpc::types::engine::PayloadStatusEnum,
};
use reth_node_builder::NodeTypes;
use reth_primitives::{stage::StageId, BlockHash, BlockNumber, Bytes, B256};
use std::{marker::PhantomData, pin::Pin};
use tokio_stream::StreamExt;

/// An helper struct to handle node actions
pub struct NodeTestContext<Node>
where
    Node: FullNodeComponents,
{
    pub inner: FullNode<Node>,
    pub payload: PayloadTestContext<Node::Engine>,
    pub network: NetworkTestContext,
    pub engine_api: EngineApiTestContext<Node::Engine>,
    pub rpc: RpcTestContext<Node>,
}

impl<Node> NodeTestContext<Node>
where
    Node: FullNodeComponents,
{
    /// Creates a new test node
    pub async fn new(node: FullNode<Node>) -> eyre::Result<Self> {
        let builder = node.payload_builder.clone();

        Ok(Self {
            inner: node.clone(),
            payload: PayloadTestContext::new(builder).await?,
            network: NetworkTestContext::new(node.network.clone()),
            engine_api: EngineApiTestContext {
                engine_api_client: node.auth_server_handle().http_client(),
                canonical_stream: node.provider.canonical_state_stream(),
                _marker: PhantomData::<Node::Engine>,
            },
            rpc: RpcTestContext { inner: node.rpc_registry },
        })
    }

    pub async fn connect(&mut self, node: &mut NodeTestContext<Node>) {
        self.network.add_peer(node.network.record()).await;
        node.network.add_peer(self.network.record()).await;
        node.network.expect_session().await;
        self.network.expect_session().await;
    }

    /// Advances the chain `length` blocks.
    ///
    /// Returns the added chain as a Vec of block hashes.
    pub async fn advance(
        &mut self,
        length: u64,
        tx_generator: impl Fn(u64) -> Pin<Box<dyn Future<Output = Bytes>>>,
        attributes_generator: impl Fn(u64) -> <Node::Engine as EngineTypes>::PayloadBuilderAttributes
            + Copy,
    ) -> eyre::Result<
        Vec<(
            <Node::Engine as EngineTypes>::BuiltPayload,
            <Node::Engine as EngineTypes>::PayloadBuilderAttributes,
        )>,
    >
    where
        <Node::Engine as EngineTypes>::ExecutionPayloadV3:
            From<<Node::Engine as EngineTypes>::BuiltPayload> + PayloadEnvelopeExt,
    {
        let mut chain = Vec::with_capacity(length as usize);
        for i in 0..length {
            let raw_tx = tx_generator(i).await;
            let tx_hash = self.rpc.inject_tx(raw_tx).await?;
            let (payload, eth_attr) = self.advance_block(vec![], attributes_generator).await?;
            let block_hash = payload.block().hash();
            let block_number = payload.block().number;
            self.assert_new_block(tx_hash, block_hash, block_number).await?;
            chain.push((payload, eth_attr));
        }
        Ok(chain)
    }

    /// Creates a new payload from given attributes generator
    /// expects a payload attribute event and waits until the payload is built.
    ///
    /// It triggers the resolve payload via engine api and expects the built payload event.
    pub async fn new_payload(
        &mut self,
        attributes_generator: impl Fn(u64) -> <Node::Engine as EngineTypes>::PayloadBuilderAttributes,
    ) -> eyre::Result<(
        <<Node as NodeTypes>::Engine as EngineTypes>::BuiltPayload,
        <<Node as NodeTypes>::Engine as EngineTypes>::PayloadBuilderAttributes,
    )>
    where
        <Node::Engine as EngineTypes>::ExecutionPayloadV3:
            From<<Node::Engine as EngineTypes>::BuiltPayload> + PayloadEnvelopeExt,
    {
        // trigger new payload building draining the pool
        let eth_attr = self.payload.new_payload(attributes_generator).await.unwrap();
        // first event is the payload attributes
        self.payload.expect_attr_event(eth_attr.clone()).await?;
        // wait for the payload builder to have finished building
        self.payload.wait_for_built_payload(eth_attr.payload_id()).await;
        // trigger resolve payload via engine api
        self.engine_api.get_payload_v3_value(eth_attr.payload_id()).await?;
        // ensure we're also receiving the built payload as event
        Ok((self.payload.expect_built_payload().await?, eth_attr))
    }

    /// Advances the node forward one block
    pub async fn advance_block(
        &mut self,
        versioned_hashes: Vec<B256>,
        attributes_generator: impl Fn(u64) -> <Node::Engine as EngineTypes>::PayloadBuilderAttributes,
    ) -> eyre::Result<(
        <Node::Engine as EngineTypes>::BuiltPayload,
        <<Node as NodeTypes>::Engine as EngineTypes>::PayloadBuilderAttributes,
    )>
    where
        <Node::Engine as EngineTypes>::ExecutionPayloadV3:
            From<<Node::Engine as EngineTypes>::BuiltPayload> + PayloadEnvelopeExt,
    {
        let (payload, eth_attr) = self.new_payload(attributes_generator).await?;

        let block_hash = self
            .engine_api
            .submit_payload(
                payload.clone(),
                eth_attr.clone(),
                PayloadStatusEnum::Valid,
                versioned_hashes,
            )
            .await?;

        // trigger forkchoice update via engine api to commit the block to the blockchain
        self.engine_api.update_forkchoice(block_hash, block_hash).await?;

        Ok((payload, eth_attr))
    }

    /// Waits for block to be available on node.
    pub async fn wait_block(
        &self,
        number: BlockNumber,
        expected_block_hash: BlockHash,
        wait_finish_checkpoint: bool,
    ) -> eyre::Result<()> {
        let mut check = !wait_finish_checkpoint;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;

            if !check && wait_finish_checkpoint {
                if let Some(checkpoint) =
                    self.inner.provider.get_stage_checkpoint(StageId::Finish)?
                {
                    if checkpoint.block_number >= number {
                        check = true
                    }
                }
            }

            if check {
                if let Some(latest_block) = self.inner.provider.block_by_number(number)? {
                    assert_eq!(latest_block.hash_slow(), expected_block_hash);
                    break
                }
                if wait_finish_checkpoint {
                    panic!("Finish checkpoint matches, but could not fetch block.");
                }
            }
        }
        Ok(())
    }

    pub async fn wait_unwind(&self, number: BlockNumber) -> eyre::Result<()> {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            if let Some(checkpoint) = self.inner.provider.get_stage_checkpoint(StageId::Headers)? {
                if checkpoint.block_number == number {
                    break
                }
            }
        }
        Ok(())
    }

    /// Asserts that a new block has been added to the blockchain
    /// and the tx has been included in the block.
    ///
    /// Does NOT work for pipeline since there's no stream notification!
    pub async fn assert_new_block(
        &mut self,
        tip_tx_hash: B256,
        block_hash: B256,
        block_number: BlockNumber,
    ) -> eyre::Result<()> {
        // get head block from notifications stream and verify the tx has been pushed to the
        // pool is actually present in the canonical block
        let head = self.engine_api.canonical_stream.next().await.unwrap();
        let tx = head.tip().transactions().next();
        assert_eq!(tx.unwrap().hash().as_slice(), tip_tx_hash.as_slice());

        loop {
            // wait for the block to commit
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if let Some(latest_block) =
                self.inner.provider.block_by_number_or_tag(BlockNumberOrTag::Latest)?
            {
                if latest_block.number == block_number {
                    // make sure the block hash we submitted via FCU engine api is the new latest
                    // block using an RPC call
                    assert_eq!(latest_block.hash_slow(), block_hash);
                    break
                }
            }
        }
        Ok(())
    }
}
