use crate::{network::NetworkTestContext, payload::PayloadTestContext, rpc::RpcTestContext};
use alloy_consensus::BlockHeader;
use alloy_eips::BlockId;
use alloy_primitives::{BlockHash, BlockNumber, Bytes, Sealable, B256};
use alloy_rpc_types_engine::ForkchoiceState;
use alloy_rpc_types_eth::BlockNumberOrTag;
use eyre::Ok;
use futures_util::Future;
use jsonrpsee::http_client::{transport::HttpBackend, HttpClient};
use reth_chainspec::EthereumHardforks;
use reth_network_api::test_utils::PeersHandleProvider;
use reth_node_api::{
    Block, BlockBody, BlockTy, EngineApiMessageVersion, EngineTypes, FullNodeComponents,
    PrimitivesTy,
};
use reth_node_builder::{rpc::RethRpcAddOns, FullNode, NodeTypesWithEngine};
use reth_node_core::primitives::SignedTransaction;
use reth_payload_primitives::{BuiltPayload, PayloadBuilderAttributes};
use reth_provider::{
    BlockReader, BlockReaderIdExt, CanonStateNotificationStream, CanonStateSubscriptions,
    StageCheckpointReader,
};
use reth_rpc_eth_api::helpers::{EthApiSpec, EthTransactions, TraceExt};
use reth_rpc_layer::AuthClientService;
use reth_stages_types::StageId;
use std::pin::Pin;
use tokio_stream::StreamExt;
use url::Url;

/// An helper struct to handle node actions
#[expect(missing_debug_implementations)]
pub struct NodeTestContext<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: RethRpcAddOns<Node>,
{
    /// The core structure representing the full node.
    pub inner: FullNode<Node, AddOns>,
    /// Context for testing payload-related features.
    pub payload: PayloadTestContext<<Node::Types as NodeTypesWithEngine>::Engine>,
    /// Context for testing network functionalities.
    pub network: NetworkTestContext<Node::Network>,
    /// Context for testing RPC features.
    pub rpc: RpcTestContext<Node, AddOns::EthApi>,
    /// Canonical state events.
    pub canonical_stream: CanonStateNotificationStream<PrimitivesTy<Node::Types>>,
}

impl<Node, Engine, AddOns> NodeTestContext<Node, AddOns>
where
    Engine: EngineTypes,
    Node: FullNodeComponents,
    Node::Types: NodeTypesWithEngine<ChainSpec: EthereumHardforks, Engine = Engine>,
    Node::Network: PeersHandleProvider,
    AddOns: RethRpcAddOns<Node>,
{
    /// Creates a new test node
    pub async fn new(
        node: FullNode<Node, AddOns>,
        attributes_generator: impl Fn(u64) -> Engine::PayloadBuilderAttributes + Send + Sync + 'static,
    ) -> eyre::Result<Self> {
        Ok(Self {
            inner: node.clone(),
            payload: PayloadTestContext::new(
                node.payload_builder_handle.clone(),
                attributes_generator,
            )
            .await?,
            network: NetworkTestContext::new(node.network.clone()),
            rpc: RpcTestContext { inner: node.add_ons_handle.rpc_registry },
            canonical_stream: node.provider.canonical_state_stream(),
        })
    }

    /// Establish a connection to the node
    pub async fn connect(&mut self, node: &mut Self) {
        self.network.add_peer(node.network.record()).await;
        node.network.next_session_established().await;
        self.network.next_session_established().await;
    }

    /// Advances the chain `length` blocks.
    ///
    /// Returns the added chain as a Vec of block hashes.
    pub async fn advance(
        &mut self,
        length: u64,
        tx_generator: impl Fn(u64) -> Pin<Box<dyn Future<Output = Bytes>>>,
    ) -> eyre::Result<Vec<Engine::BuiltPayload>>
    where
        AddOns::EthApi: EthApiSpec<Provider: BlockReader<Block = BlockTy<Node::Types>>>
            + EthTransactions
            + TraceExt,
    {
        let mut chain = Vec::with_capacity(length as usize);
        for i in 0..length {
            let raw_tx = tx_generator(i).await;
            let tx_hash = self.rpc.inject_tx(raw_tx).await?;
            let payload = self.advance_block().await?;
            let block_hash = payload.block().hash();
            let block_number = payload.block().number();
            self.assert_new_block(tx_hash, block_hash, block_number).await?;
            chain.push(payload);
        }
        Ok(chain)
    }

    /// Creates a new payload from given attributes generator
    /// expects a payload attribute event and waits until the payload is built.
    ///
    /// It triggers the resolve payload via engine api and expects the built payload event.
    pub async fn new_payload(&mut self) -> eyre::Result<Engine::BuiltPayload> {
        // trigger new payload building draining the pool
        let eth_attr = self.payload.new_payload().await.unwrap();
        // first event is the payload attributes
        self.payload.expect_attr_event(eth_attr.clone()).await?;
        // wait for the payload builder to have finished building
        self.payload.wait_for_built_payload(eth_attr.payload_id()).await;
        // ensure we're also receiving the built payload as event
        Ok(self.payload.expect_built_payload().await?)
    }

    /// Triggers payload building job and submits it to the engine.
    pub async fn build_and_submit_payload(&mut self) -> eyre::Result<Engine::BuiltPayload> {
        let payload = self.new_payload().await?;

        self.submit_payload(payload.clone()).await?;

        Ok(payload)
    }

    /// Advances the node forward one block
    pub async fn advance_block(&mut self) -> eyre::Result<Engine::BuiltPayload> {
        let payload = self.build_and_submit_payload().await?;

        // trigger forkchoice update via engine api to commit the block to the blockchain
        self.update_forkchoice(payload.block().hash(), payload.block().hash()).await?;

        Ok(payload)
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
                    assert_eq!(latest_block.header().hash_slow(), expected_block_hash);
                    break
                }
                assert!(
                    !wait_finish_checkpoint,
                    "Finish checkpoint matches, but could not fetch block."
                );
            }
        }
        Ok(())
    }

    /// Waits for the node to unwind to the given block number
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
        let head = self.canonical_stream.next().await.unwrap();
        let tx = head.tip().body().transactions().first();
        assert_eq!(tx.unwrap().tx_hash().as_slice(), tip_tx_hash.as_slice());

        loop {
            // wait for the block to commit
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if let Some(latest_block) =
                self.inner.provider.block_by_number_or_tag(BlockNumberOrTag::Latest)?
            {
                if latest_block.header().number() == block_number {
                    // make sure the block hash we submitted via FCU engine api is the new latest
                    // block using an RPC call
                    assert_eq!(latest_block.header().hash_slow(), block_hash);
                    break
                }
            }
        }
        Ok(())
    }

    /// Gets block hash by number.
    pub fn block_hash(&self, number: u64) -> BlockHash {
        self.inner
            .provider
            .sealed_header_by_number_or_tag(BlockNumberOrTag::Number(number))
            .unwrap()
            .unwrap()
            .hash()
    }

    /// Sends FCU and waits for the node to sync to the given block.
    pub async fn sync_to(&self, block: BlockHash) -> eyre::Result<()> {
        let start = std::time::Instant::now();

        while self
            .inner
            .provider
            .sealed_header_by_id(BlockId::Number(BlockNumberOrTag::Latest))?
            .is_none_or(|h| h.hash() != block)
        {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            self.update_forkchoice(block, block).await?;

            assert!(start.elapsed() <= std::time::Duration::from_secs(40), "timed out");
        }

        // Hack to make sure that all components have time to process canonical state update.
        // Otherwise, this might result in e.g "nonce too low" errors when advancing chain further,
        // making tests flaky.
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

        Ok(())
    }

    /// Sends a forkchoice update message to the engine.
    pub async fn update_forkchoice(&self, current_head: B256, new_head: B256) -> eyre::Result<()> {
        self.inner
            .add_ons_handle
            .beacon_engine_handle
            .fork_choice_updated(
                ForkchoiceState {
                    head_block_hash: new_head,
                    safe_block_hash: current_head,
                    finalized_block_hash: current_head,
                },
                None,
                EngineApiMessageVersion::default(),
            )
            .await?;

        Ok(())
    }

    /// Sends forkchoice update to the engine api with a zero finalized hash
    pub async fn update_optimistic_forkchoice(&self, hash: B256) -> eyre::Result<()> {
        self.update_forkchoice(B256::ZERO, hash).await
    }

    /// Submits a payload to the engine.
    pub async fn submit_payload(&self, payload: Engine::BuiltPayload) -> eyre::Result<B256> {
        let block_hash = payload.block().hash();
        self.inner
            .add_ons_handle
            .beacon_engine_handle
            .new_payload(Engine::block_to_payload(payload.block().clone()))
            .await?;

        Ok(block_hash)
    }

    /// Returns the RPC URL.
    pub fn rpc_url(&self) -> Url {
        let addr = self.inner.rpc_server_handle().http_local_addr().unwrap();
        format!("http://{}", addr).parse().unwrap()
    }

    /// Returns an RPC client.
    pub fn rpc_client(&self) -> Option<HttpClient> {
        self.inner.rpc_server_handle().http_client()
    }

    /// Returns an Engine API client.
    pub fn engine_api_client(&self) -> HttpClient<AuthClientService<HttpBackend>> {
        self.inner.auth_server_handle().http_client()
    }
}
