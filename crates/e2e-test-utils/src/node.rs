use crate::{network::NetworkTestContext, payload::PayloadTestContext, rpc::RpcTestContext};
use alloy_consensus::{transaction::TxHashRef, BlockHeader};
use alloy_eips::BlockId;
use alloy_network::{Ethereum, IntoWallet};
use alloy_primitives::{BlockHash, BlockNumber, Bytes, Sealable, B256};
use alloy_provider::{
    fillers::{FillProvider, RecommendedFillers, TxFiller},
    ProviderBuilder, RootProvider,
};
use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV5, ForkchoiceState};
use alloy_rpc_types_eth::BlockNumberOrTag;
use eyre::{eyre, Ok};
use futures_util::Future;
use jsonrpsee::{core::client::ClientT, http_client::HttpClient};
use reth_chainspec::EthereumHardforks;
use reth_network_api::test_utils::PeersHandleProvider;
use reth_node_api::{Block, BlockBody, BlockTy, FullNodeComponents, PayloadTypes, PrimitivesTy};
use reth_node_builder::{rpc::RethRpcAddOns, FullNode, NodeTypes};
use reth_payload_primitives::BuiltPayload;
use reth_provider::{
    BlockReader, BlockReaderIdExt, CanonStateNotificationStream, CanonStateSubscriptions,
    HeaderProvider, StageCheckpointReader,
};
use reth_rpc_api::TestingBuildBlockRequestV1;
use reth_rpc_builder::auth::AuthServerHandle;
use reth_rpc_eth_api::helpers::{EthApiSpec, EthTransactions, TraceExt};
use reth_stages_types::StageId;
use std::{pin::Pin, time::Duration};
use tokio_stream::StreamExt;
use url::Url;

/// Maximum time the wait helpers of [`NodeTestContext`] wait for the node, e.g. to sync to or
/// commit a block.
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// A helper struct to handle node actions
#[expect(missing_debug_implementations)]
pub struct NodeTestContext<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: RethRpcAddOns<Node>,
{
    /// The core structure representing the full node.
    pub inner: FullNode<Node, AddOns>,
    /// Context for testing payload-related features.
    pub payload: PayloadTestContext<<Node::Types as NodeTypes>::Payload>,
    /// Context for testing network functionalities.
    pub network: NetworkTestContext<Node::Network>,
    /// Context for testing RPC features.
    pub rpc: RpcTestContext<Node, AddOns::EthApi>,
    /// Canonical state events.
    pub canonical_stream: CanonStateNotificationStream<PrimitivesTy<Node::Types>>,
}

impl<Node, Payload, AddOns> NodeTestContext<Node, AddOns>
where
    Payload: PayloadTypes,
    Node: FullNodeComponents,
    Node::Types: NodeTypes<ChainSpec: EthereumHardforks, Payload = Payload>,
    Node::Network: PeersHandleProvider,
    AddOns: RethRpcAddOns<Node>,
{
    /// Creates a new test node
    ///
    /// Payload timestamps start after the default Cancun timestamp of the payload context, or after
    /// the timestamp of the latest block if it is newer, e.g. for a custom genesis or an imported
    /// chain.
    pub async fn new(
        node: FullNode<Node, AddOns>,
        attributes_generator: impl Fn(u64) -> Payload::PayloadAttributes + Send + Sync + 'static,
    ) -> eyre::Result<Self> {
        let mut payload =
            PayloadTestContext::new(node.payload_builder_handle.clone(), attributes_generator)
                .await?;
        if let Some(latest) =
            node.provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Latest)?
        {
            payload.timestamp = payload.timestamp.max(latest.timestamp());
        }
        Ok(Self {
            inner: node.clone(),
            payload,
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
    ) -> eyre::Result<Vec<Payload::BuiltPayload>>
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

    /// Returns the current forkchoice state of the node.
    pub fn current_forkchoice_state(&self) -> eyre::Result<ForkchoiceState> {
        let latest_header =
            self.inner.provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap();

        if latest_header.number() == 0 {
            return Ok(ForkchoiceState::same_hash(latest_header.hash()));
        }

        Ok(ForkchoiceState {
            head_block_hash: latest_header.hash(),
            safe_block_hash: self
                .inner
                .provider
                .sealed_header_by_number_or_tag(BlockNumberOrTag::Safe)?
                .unwrap()
                .hash(),
            finalized_block_hash: self
                .inner
                .provider
                .sealed_header_by_number_or_tag(BlockNumberOrTag::Finalized)?
                .unwrap()
                .hash(),
        })
    }

    /// Creates a new payload from given attributes generator
    /// expects a payload attribute event and waits until the payload is built.
    ///
    /// It triggers the resolve payload via engine api and expects the built payload event.
    pub async fn new_payload(&mut self) -> eyre::Result<Payload::BuiltPayload> {
        let eth_attr = self.payload.next_attributes();
        let payload_id = self
            .inner
            .add_ons_handle
            .beacon_engine_handle
            .fork_choice_updated(self.current_forkchoice_state()?, Some(eth_attr.clone()))
            .await?
            .payload_id
            .unwrap();
        // first event is the payload attributes
        self.payload.expect_attr_event(eth_attr).await?;
        // wait for the payload builder to have finished building
        self.payload.wait_for_built_payload(payload_id).await;
        // ensure we're also receiving the built payload as event
        Ok(self.payload.expect_built_payload().await?)
    }

    /// Triggers payload building job and submits it to the engine.
    pub async fn build_and_submit_payload(&mut self) -> eyre::Result<Payload::BuiltPayload> {
        let payload = self.new_payload().await?;

        self.submit_payload(payload.clone()).await?;

        Ok(payload)
    }

    /// Advances the node forward one block
    pub async fn advance_block(&mut self) -> eyre::Result<Payload::BuiltPayload> {
        let payload = self.build_and_submit_payload().await?;

        // trigger forkchoice update via engine api to commit the block to the blockchain
        self.update_forkchoice(payload.block().hash(), payload.block().hash()).await?;

        Ok(payload)
    }

    /// Waits for block to be available on node.
    ///
    /// Returns an error if the block is not available within [`WAIT_TIMEOUT`].
    pub async fn wait_block(
        &self,
        number: BlockNumber,
        expected_block_hash: BlockHash,
        wait_finish_checkpoint: bool,
    ) -> eyre::Result<()> {
        let wait = async {
            let mut check = !wait_finish_checkpoint;
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;

                if !check &&
                    wait_finish_checkpoint &&
                    let Some(checkpoint) =
                        self.inner.provider.get_stage_checkpoint(StageId::Finish)? &&
                    checkpoint.block_number >= number
                {
                    check = true
                }

                if check {
                    if let Some(latest_header) = self.inner.provider.header_by_number(number)? {
                        assert_eq!(latest_header.hash_slow(), expected_block_hash);
                        break
                    }
                    assert!(
                        !wait_finish_checkpoint,
                        "Finish checkpoint matches, but could not fetch block."
                    );
                }
            }
            Ok(())
        };
        tokio::time::timeout(WAIT_TIMEOUT, wait)
            .await
            .map_err(|_| eyre!("timed out waiting for block {number}"))?
    }

    /// Waits for the node to unwind to the given block number.
    ///
    /// Returns an error if the node does not unwind within [`WAIT_TIMEOUT`].
    pub async fn wait_unwind(&self, number: BlockNumber) -> eyre::Result<()> {
        let wait = async {
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                if let Some(checkpoint) =
                    self.inner.provider.get_stage_checkpoint(StageId::Headers)? &&
                    checkpoint.block_number == number
                {
                    break
                }
            }
            Ok(())
        };
        tokio::time::timeout(WAIT_TIMEOUT, wait)
            .await
            .map_err(|_| eyre!("timed out waiting for unwind to block {number}"))?
    }

    /// Asserts that a new block has been added to the blockchain
    /// and the tx has been included in the block.
    ///
    /// Does NOT work for pipeline since there's no stream notification! Returns an error if the
    /// block is not committed within [`WAIT_TIMEOUT`].
    pub async fn assert_new_block(
        &mut self,
        tip_tx_hash: B256,
        block_hash: B256,
        block_number: BlockNumber,
    ) -> eyre::Result<()> {
        let wait =
            async {
                // get head block from notifications stream and verify the tx has been pushed to the
                // pool is actually present in the canonical block
                let head = self
                    .canonical_stream
                    .next()
                    .await
                    .ok_or_else(|| eyre!("canonical state stream closed"))?;
                let tx =
                    head.tip().body().transactions().first().ok_or_else(|| {
                        eyre!("block {} has no transactions", head.tip().number())
                    })?;
                assert_eq!(tx.tx_hash().as_slice(), tip_tx_hash.as_slice());

                loop {
                    // wait for the block to commit
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    if let Some(latest_block) =
                        self.inner.provider.block_by_number_or_tag(BlockNumberOrTag::Latest)? &&
                        latest_block.header().number() == block_number
                    {
                        // make sure the block hash we submitted via FCU engine api is the new
                        // latest block using an RPC call
                        assert_eq!(latest_block.header().hash_slow(), block_hash);
                        break
                    }
                }
                Ok(())
            };
        tokio::time::timeout(WAIT_TIMEOUT, wait)
            .await
            .map_err(|_| eyre!("timed out waiting for block {block_number} to be committed"))?
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
    ///
    /// Returns an error if the node does not sync within [`WAIT_TIMEOUT`].
    pub async fn sync_to(&self, block: BlockHash) -> eyre::Result<()> {
        let sync = async {
            while self
                .inner
                .provider
                .sealed_header_by_id(BlockId::Number(BlockNumberOrTag::Latest))?
                .is_none_or(|h| h.hash() != block)
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
                self.update_forkchoice(block, block).await?;
            }
            Ok(())
        };
        tokio::time::timeout(WAIT_TIMEOUT, sync)
            .await
            .map_err(|_| eyre!("timed out syncing to block {block}"))??;

        // Hack to make sure that all components have time to process canonical state update.
        // Otherwise, this might result in e.g "nonce too low" errors when advancing chain further,
        // making tests flaky.
        tokio::time::sleep(Duration::from_millis(1000)).await;

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
            )
            .await?;

        Ok(())
    }

    /// Sends forkchoice update to the engine api with a zero finalized hash
    pub async fn update_optimistic_forkchoice(&self, hash: B256) -> eyre::Result<()> {
        self.update_forkchoice(B256::ZERO, hash).await
    }

    /// Submits a payload to the engine.
    pub async fn submit_payload(&self, payload: Payload::BuiltPayload) -> eyre::Result<B256> {
        let block_hash = payload.block().hash();
        self.inner.add_ons_handle.beacon_engine_handle.new_payload(payload.into()).await?;

        Ok(block_hash)
    }

    /// Returns the RPC URL.
    pub fn rpc_url(&self) -> Url {
        let addr = self.inner.rpc_server_handle().http_local_addr().unwrap();
        format!("http://{addr}").parse().unwrap()
    }

    /// Returns an RPC client.
    pub fn rpc_client(&self) -> Option<HttpClient> {
        self.inner.rpc_server_handle().http_client()
    }

    /// Returns an alloy provider with the recommended fillers connected to the HTTP RPC server.
    ///
    /// # Panics
    ///
    /// If the HTTP RPC server is disabled.
    pub fn rpc_provider(
        &self,
    ) -> FillProvider<impl TxFiller<Ethereum> + use<Node, Payload, AddOns>, RootProvider> {
        self.rpc_provider_for::<Ethereum>()
    }

    /// Returns an alloy provider with the recommended fillers and the given wallet connected to
    /// the HTTP RPC server.
    ///
    /// Transactions sent via the provider are signed by the wallet, e.g. a
    /// [`PrivateKeySigner`](alloy_signer_local::PrivateKeySigner) of the test
    /// [`Wallet`](crate::wallet::Wallet).
    ///
    /// # Panics
    ///
    /// If the HTTP RPC server is disabled.
    pub fn rpc_provider_with_wallet<W>(
        &self,
        wallet: W,
    ) -> FillProvider<impl TxFiller<Ethereum> + use<W, Node, Payload, AddOns>, RootProvider>
    where
        W: IntoWallet<Ethereum, NetworkWallet: Clone>,
    {
        self.rpc_provider_with_wallet_for::<Ethereum, W>(wallet)
    }

    /// Returns an alloy provider for the network `Net` with its recommended fillers connected to
    /// the HTTP RPC server.
    ///
    /// This is [`Self::rpc_provider`] for nodes whose RPC types differ from Ethereum's.
    ///
    /// # Panics
    ///
    /// If the HTTP RPC server is disabled.
    pub fn rpc_provider_for<Net: RecommendedFillers>(
        &self,
    ) -> FillProvider<impl TxFiller<Net> + use<Net, Node, Payload, AddOns>, RootProvider<Net>, Net>
    {
        ProviderBuilder::new_with_network::<Net>().connect_http(self.rpc_url())
    }

    /// Returns an alloy provider for the network `Net` with its recommended fillers and the given
    /// wallet connected to the HTTP RPC server.
    ///
    /// This is [`Self::rpc_provider_with_wallet`] for nodes whose RPC types differ from
    /// Ethereum's.
    ///
    /// # Panics
    ///
    /// If the HTTP RPC server is disabled.
    pub fn rpc_provider_with_wallet_for<Net, W>(
        &self,
        wallet: W,
    ) -> FillProvider<impl TxFiller<Net> + use<Net, W, Node, Payload, AddOns>, RootProvider<Net>, Net>
    where
        Net: RecommendedFillers,
        W: IntoWallet<Net, NetworkWallet: Clone>,
    {
        ProviderBuilder::new_with_network::<Net>().wallet(wallet).connect_http(self.rpc_url())
    }

    /// Returns an Engine API client.
    pub fn auth_server_handle(&self) -> AuthServerHandle {
        self.inner.auth_server_handle().clone()
    }

    /// Creates a [`crate::testsuite::NodeClient`] from this test context.
    ///
    /// This helper method extracts the necessary handles and creates a client
    /// that can interact with both the regular RPC and Engine API endpoints.
    /// It automatically includes the beacon engine handle for direct consensus engine interaction.
    pub fn to_node_client(&self) -> eyre::Result<crate::testsuite::NodeClient<Payload>> {
        let rpc = self
            .rpc_client()
            .ok_or_else(|| eyre::eyre!("Failed to create HTTP RPC client for node"))?;
        let auth = self.auth_server_handle();
        let url = self.rpc_url();
        let beacon_handle = self.inner.add_ons_handle.beacon_engine_handle.clone();

        let mut client =
            crate::testsuite::NodeClient::new_with_beacon_engine(rpc, auth, url, beacon_handle);
        client.payload_builder = Some(self.inner.payload_builder_handle.clone());
        Ok(client)
    }

    /// Calls the `testing_buildBlockV1` RPC on this node.
    ///
    /// This endpoint builds a block using the provided parent, payload attributes, and
    /// transactions. Requires the `Testing` RPC module to be enabled.
    pub async fn testing_build_block_v1(
        &self,
        request: TestingBuildBlockRequestV1,
    ) -> eyre::Result<ExecutionPayloadEnvelopeV5> {
        let client =
            self.rpc_client().ok_or_else(|| eyre::eyre!("HTTP RPC client not available"))?;

        let res: ExecutionPayloadEnvelopeV5 =
            client.request("testing_buildBlockV1", request.into_params()).await?;
        eyre::Ok(res)
    }
}
