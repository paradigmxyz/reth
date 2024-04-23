use crate::{
    engine_api::EngineApiTestContext, network::NetworkTestContext, payload::PayloadTestContext,
    rpc::RpcTestContext, traits::PayloadEnvelopeExt,
};
use alloy_consensus::{TxEip4844Variant, TxEnvelope};
use alloy_rpc_types::BlockNumberOrTag;
use eyre::Ok;
use reth::{
    api::{BuiltPayload, EngineTypes, FullNodeComponents, NodeTypes, PayloadBuilderAttributes},
    builder::FullNode,
    providers::{BlockReaderIdExt, CanonStateSubscriptions},
};

use reth_primitives::{constants::eip4844::MAINNET_KZG_TRUSTED_SETUP, BlockNumber, B256};
use std::marker::PhantomData;
use tokio_stream::StreamExt;

/// An helper struct to handle node actions
pub struct NodeTestContext<Node>
where
    Node: FullNodeComponents,
{
    pub inner: FullNode<Node>,
    payload: PayloadTestContext<Node::Engine>,
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
            network: NetworkTestContext::new(node.network.clone()),
            payload: PayloadTestContext::new(builder).await?,
            engine_api: EngineApiTestContext {
                engine_api_client: node.auth_server_handle().http_client(),
                canonical_stream: node.provider.canonical_state_stream(),
                _marker: PhantomData::<Node::Engine>,
            },
            rpc: RpcTestContext { inner: node.rpc_registry.clone() },
        })
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
        self.engine_api.get_payload_v3(eth_attr.payload_id()).await?;
        // ensure we're also receiving the built payload as event
        Ok((self.payload.expect_built_payload().await?, eth_attr))
    }

    /// Advances the node forward by creating a new payload, submitting it via engine api
    /// and triggering a forkchoice update via engine api.
    pub async fn advance(
        &mut self,
        versioned_hashes: Vec<B256>,
        attributes_generator: impl Fn(u64) -> <Node::Engine as EngineTypes>::PayloadBuilderAttributes,
    ) -> eyre::Result<(B256, u64)>
    where
        <Node::Engine as EngineTypes>::ExecutionPayloadV3:
            From<<Node::Engine as EngineTypes>::BuiltPayload> + PayloadEnvelopeExt,
    {
        let (payload, eth_attr) = self.new_payload(attributes_generator).await?;

        // submit payload via engine api
        let block_number = payload.block().number;
        let block_hash =
            self.engine_api.submit_payload(payload, eth_attr.clone(), versioned_hashes).await?;

        // trigger forkchoice update via engine api to commit the block to the blockchain
        self.engine_api.update_forkchoice(block_hash, block_hash).await?;

        Ok((block_hash, block_number))
    }

    /// Validates the sidecar of a given tx envelope and returns the versioned hashes
    pub fn validate_sidecar(&self, tx: TxEnvelope) -> Vec<B256> {
        let proof_setting = MAINNET_KZG_TRUSTED_SETUP.clone();

        match tx {
            TxEnvelope::Eip4844(signed) => match signed.tx() {
                TxEip4844Variant::TxEip4844WithSidecar(tx) => {
                    tx.validate_blob(&proof_setting).unwrap();
                    tx.sidecar.versioned_hashes().collect()
                }
                _ => panic!("Expected Eip4844 transaction with sidecar"),
            },
            _ => panic!("Expected Eip4844 transaction"),
        }
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
        let tx = head.tip().transactions().next().unwrap();
        assert_eq!(tx.hash().as_slice(), tip_tx_hash.as_slice());

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
