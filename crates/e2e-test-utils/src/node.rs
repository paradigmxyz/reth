use crate::{
    engine_api::EngineApiHelper, network::NetworkHelper, payload::PayloadHelper,
    traits::PayloadEnvelopeExt,
};
use alloy_rpc_types::BlockNumberOrTag;
use eyre::Ok;
use futures_util::Future;
use reth::{
    api::{BuiltPayload, EngineTypes, FullNodeComponents, PayloadBuilderAttributes},
    builder::FullNode,
    providers::{BlockReader, BlockReaderIdExt, CanonStateSubscriptions, StageCheckpointReader},
    rpc::{
        eth::{error::EthResult, EthTransactions},
        types::engine::PayloadStatusEnum,
    },
};
use reth_primitives::{stage::StageId, BlockHash, BlockNumber, Bytes, B256};
use std::{marker::PhantomData, pin::Pin};
use tokio_stream::StreamExt;

/// An helper struct to handle node actions
pub struct NodeHelper<Node>
where
    Node: FullNodeComponents,
{
    pub inner: FullNode<Node>,
    pub payload: PayloadHelper<Node::Engine>,
    pub network: NetworkHelper,
    pub engine_api: EngineApiHelper<Node::Engine>,
}

impl<Node> NodeHelper<Node>
where
    Node: FullNodeComponents,
{
    /// Creates a new test node
    pub async fn new(node: FullNode<Node>) -> eyre::Result<Self> {
        let builder = node.payload_builder.clone();

        Ok(Self {
            inner: node.clone(),
            network: NetworkHelper::new(node.network.clone()),
            payload: PayloadHelper::new(builder).await?,
            engine_api: EngineApiHelper {
                engine_api_client: node.auth_server_handle().http_client(),
                canonical_stream: node.provider.canonical_state_stream(),
                _marker: PhantomData::<Node::Engine>,
            },
        })
    }

    pub async fn connect(&mut self, node: &mut NodeHelper<Node>) {
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
        tx_generator: impl Fn() -> Pin<Box<dyn Future<Output = Bytes>>>,
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
        for _ in 0..length {
            let (payload, _) =
                self.advance_block(tx_generator().await, attributes_generator).await?;
            chain.push(payload);
        }
        Ok(chain)
    }

    /// Advances the node forward one block
    pub async fn advance_block(
        &mut self,
        raw_tx: Bytes,
        attributes_generator: impl Fn(u64) -> <Node::Engine as EngineTypes>::PayloadBuilderAttributes,
    ) -> eyre::Result<(
        (
            <Node::Engine as EngineTypes>::BuiltPayload,
            <Node::Engine as EngineTypes>::PayloadBuilderAttributes,
        ),
        B256,
    )>
    where
        <Node::Engine as EngineTypes>::ExecutionPayloadV3:
            From<<Node::Engine as EngineTypes>::BuiltPayload> + PayloadEnvelopeExt,
    {
        // push tx into pool via RPC server
        let tx_hash = self.inject_tx(raw_tx).await?;

        // trigger new payload building draining the pool
        let eth_attr = self.payload.new_payload(attributes_generator).await.unwrap();

        // first event is the payload attributes
        self.payload.expect_attr_event(eth_attr.clone()).await?;

        // wait for the payload builder to have finished building
        self.payload.wait_for_built_payload(eth_attr.payload_id()).await;

        // trigger resolve payload via engine api
        self.engine_api.get_payload_v3(eth_attr.payload_id()).await?;

        // ensure we're also receiving the built payload as event
        let payload = self.payload.expect_built_payload().await?;

        // submit payload via engine api
        let block_hash = self
            .engine_api
            .submit_payload(payload.clone(), eth_attr.clone(), PayloadStatusEnum::Valid)
            .await?;

        // trigger forkchoice update via engine api to commit the block to the blockchain
        self.engine_api.update_forkchoice(block_hash).await?;

        // assert the block has been committed to the blockchain
        self.assert_new_block(tx_hash, block_hash, payload.block().number).await?;
        Ok(((payload, eth_attr), tx_hash))
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
                    if latest_block.hash_slow() != expected_block_hash {
                        // TODO: only if its awaiting a reorg
                        continue
                    }
                    break
                }
                if wait_finish_checkpoint {
                    panic!("Finish checkpoint matches, but could not fetch block.");
                }
            }
        }
        Ok(())
    }

    /// Injects a raw transaction into the node tx pool via RPC server
    async fn inject_tx(&mut self, raw_tx: Bytes) -> EthResult<B256> {
        let eth_api = self.inner.rpc_registry.eth_api();
        eth_api.send_raw_transaction(raw_tx).await
    }

    /// Asserts that a new block has been added to the blockchain
    /// and the tx has been included in the block
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
