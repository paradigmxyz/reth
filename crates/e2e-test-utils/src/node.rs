use crate::{
    network::NetworkTestContext,
    payload::PayloadTestContext,
    rpc::RpcTestContext,
    wait::{poll_until, WAIT_TIMEOUT},
};
use alloy_consensus::{transaction::TxHashRef, BlockHeader};
use alloy_eips::BlockId;
use alloy_network::{Ethereum, IntoWallet};
use alloy_primitives::{BlockHash, BlockNumber, Bytes, Sealable, B256};
use alloy_provider::{
    fillers::{FillProvider, RecommendedFillers, TxFiller},
    Provider, ProviderBuilder, RootProvider,
};
use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV5, ForkchoiceState, ForkchoiceUpdated};
use alloy_rpc_types_eth::BlockNumberOrTag;
use eyre::{ensure, eyre, Ok};
use futures_util::Future;
use jsonrpsee::{core::client::ClientT, http_client::HttpClient};
use reth_chainspec::EthereumHardforks;
use reth_network_api::test_utils::PeersHandleProvider;
use reth_node_api::{Block, BlockBody, BlockTy, FullNodeComponents, PayloadTypes, PrimitivesTy};
use reth_node_builder::{rpc::RethRpcAddOns, FullNode, NodeTypes};
use reth_payload_primitives::BuiltPayload;
use reth_provider::{
    BlockNumReader, BlockReader, BlockReaderIdExt, CanonStateNotificationStream,
    CanonStateSubscriptions, DatabaseProviderFactory, HeaderProvider, StageCheckpointReader,
};
use reth_rpc_api::TestingBuildBlockRequestV1;
use reth_rpc_builder::auth::AuthServerHandle;
use reth_rpc_eth_api::helpers::{EthApiSpec, EthTransactions, TraceExt};
use reth_stages_types::StageId;
use std::{pin::Pin, sync::Arc, time::Duration};
use tokio_stream::StreamExt;
use url::Url;

/// Interval at which the alloy providers of [`NodeTestContext`] poll the node, e.g. for receipts
/// of pending transactions.
///
/// Much shorter than alloy's default for local nodes, since test nodes build blocks on demand or
/// with short dev block times.
pub const RPC_PROVIDER_POLL_INTERVAL: Duration = Duration::from_millis(10);

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

    /// Injects the raw transaction into the pool and advances the node one block, returning the
    /// transaction hash and the built payload.
    ///
    /// Returns an error if the transaction is not included in the block.
    pub async fn inject_and_advance(
        &mut self,
        raw_tx: Bytes,
    ) -> eyre::Result<(B256, Payload::BuiltPayload)>
    where
        AddOns::EthApi: EthApiSpec<Provider: BlockReader<Block = BlockTy<Node::Types>>>
            + EthTransactions
            + TraceExt,
    {
        let tx_hash = self.rpc.inject_tx(raw_tx).await?;
        let payload = self.advance_block().await?;
        let block = payload.block();
        ensure!(
            block.body().transactions().iter().any(|tx| *tx.tx_hash() == tx_hash),
            "transaction {tx_hash} was not included in block {}",
            block.number()
        );
        Ok((tx_hash, payload))
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
        let provider = &self.inner.provider;
        poll_until(format_args!("block {number}"), move || async move {
            if wait_finish_checkpoint &&
                provider
                    .get_stage_checkpoint(StageId::Finish)?
                    .is_none_or(|checkpoint| checkpoint.block_number < number)
            {
                return Ok(None)
            }
            let Some(header) = provider.header_by_number(number)? else {
                ensure!(
                    !wait_finish_checkpoint,
                    "Finish checkpoint matches, but could not fetch block {number}"
                );
                return Ok(None)
            };
            let hash = header.hash_slow();
            ensure!(
                hash == expected_block_hash,
                "block {number} is {hash}, expected {expected_block_hash}"
            );
            Ok(Some(()))
        })
        .await
    }

    /// Waits for the node to unwind to the given block number.
    ///
    /// Returns an error if the node does not unwind within [`WAIT_TIMEOUT`].
    pub async fn wait_unwind(&self, number: BlockNumber) -> eyre::Result<()> {
        let provider = &self.inner.provider;
        poll_until(format_args!("unwind to block {number}"), move || async move {
            let checkpoint = provider.get_stage_checkpoint(StageId::Headers)?;
            Ok(checkpoint.is_some_and(|checkpoint| checkpoint.block_number == number).then_some(()))
        })
        .await
    }

    /// Waits until `condition` holds for the transaction pool of the node.
    ///
    /// Returns an error if the condition does not hold within [`WAIT_TIMEOUT`].
    pub async fn wait_for_pool(
        &self,
        mut condition: impl FnMut(&Node::Pool) -> bool,
    ) -> eyre::Result<()> {
        let pool = &self.inner.pool;
        poll_until("transaction pool condition", move || {
            let ready = condition(pool);
            async move { Ok(ready.then_some(())) }
        })
        .await
    }

    /// Asserts that a new block has been added to the blockchain and the tx has been included in
    /// the block, at any position.
    ///
    /// Does NOT work for pipeline since there's no stream notification! Returns an error if the
    /// block is not committed within [`WAIT_TIMEOUT`].
    pub async fn assert_new_block(
        &mut self,
        tip_tx_hash: B256,
        block_hash: B256,
        block_number: BlockNumber,
    ) -> eyre::Result<()> {
        // get head block from notifications stream and verify the tx has been pushed to the pool is
        // actually present in the canonical block
        let head = tokio::time::timeout(WAIT_TIMEOUT, self.canonical_stream.next())
            .await
            .map_err(|_| eyre!("timed out waiting for block {block_number}"))?
            .ok_or_else(|| eyre!("canonical state stream closed"))?;
        ensure!(
            head.tip().body().transactions().iter().any(|tx| *tx.tx_hash() == tip_tx_hash),
            "transaction {tip_tx_hash} is not included in block {}",
            head.tip().number()
        );

        // wait for the block to commit and make sure the block hash we submitted via FCU engine
        // api is the new latest block
        let provider = &self.inner.provider;
        poll_until(format_args!("block {block_number} to be committed"), move || async move {
            let Some(latest) = provider.block_by_number_or_tag(BlockNumberOrTag::Latest)? else {
                return Ok(None)
            };
            if latest.header().number() != block_number {
                return Ok(None)
            }
            let hash = latest.header().hash_slow();
            ensure!(
                hash == block_hash,
                "latest block {block_number} is {hash}, expected {block_hash}"
            );
            Ok(Some(()))
        })
        .await
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

    /// Sends a forkchoice update message to the engine and returns its response.
    pub async fn update_forkchoice(
        &self,
        current_head: B256,
        new_head: B256,
    ) -> eyre::Result<ForkchoiceUpdated> {
        Ok(self
            .inner
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
            .await?)
    }

    /// Sends forkchoice update to the engine api with a zero finalized hash and returns its
    /// response.
    pub async fn update_optimistic_forkchoice(
        &self,
        hash: B256,
    ) -> eyre::Result<ForkchoiceUpdated> {
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
    /// the HTTP RPC server, polling every [`RPC_PROVIDER_POLL_INTERVAL`].
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
        let provider = ProviderBuilder::new_with_network::<Net>().connect_http(self.rpc_url());
        provider.client().set_poll_interval(RPC_PROVIDER_POLL_INTERVAL);
        provider
    }

    /// Returns an alloy provider for the network `Net` with its recommended fillers and the given
    /// wallet connected to the HTTP RPC server, polling every [`RPC_PROVIDER_POLL_INTERVAL`].
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
        let provider =
            ProviderBuilder::new_with_network::<Net>().wallet(wallet).connect_http(self.rpc_url());
        provider.client().set_poll_interval(RPC_PROVIDER_POLL_INTERVAL);
        provider
    }

    /// Returns an Engine API client.
    pub fn auth_server_handle(&self) -> AuthServerHandle {
        self.inner.auth_server_handle().clone()
    }

    /// Creates a [`crate::testsuite::NodeClient`] from this test context.
    ///
    /// This helper method extracts the necessary handles and creates a client
    /// that can interact with both the regular RPC and Engine API endpoints.
    /// It automatically includes the beacon engine handle for direct consensus engine interaction
    /// and read-only access to the node's database.
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
        let provider = self.inner.provider.clone();
        client.database = Some(Arc::new(move || {
            provider.database_provider_ro().map(|db| Box::new(db) as Box<dyn BlockNumReader>)
        }));
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
