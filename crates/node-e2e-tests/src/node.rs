use alloy_rpc_types::BlockNumberOrTag;
use eyre::Ok;
use jsonrpsee::http_client::HttpClient;
use reth::{
    args::{DiscoveryArgs, NetworkArgs},
    blockchain_tree::ShareableBlockchainTree,
    builder::{
        components::FullNodeComponentsAdapter, FullNode, FullNodeTypesAdapter, NodeBuilder,
        NodeHandle,
    },
    network::{NetworkEvent, NetworkEvents, NetworkHandle, PeersInfo},
    providers::{
        providers::BlockchainProvider, BlockReaderIdExt, CanonStateNotificationStream,
        CanonStateSubscriptions,
    },
    revm::EvmProcessorFactory,
    rpc::{
        api::EngineApiClient,
        eth::{error::EthResult, EthTransactions},
        types::engine::{ExecutionPayloadEnvelopeV3, ForkchoiceState, PayloadAttributes},
    },
    tasks::TaskExecutor,
    transaction_pool::{
        blobstore::DiskFileBlobStore, CoinbaseTipOrdering, EthPooledTransaction,
        EthTransactionValidator, Pool, TransactionValidationTaskExecutor,
    },
};
use reth_db::{test_utils::TempDatabase, DatabaseEnv};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::{EthEngineTypes, EthEvmConfig, EthereumNode};
use reth_payload_builder::{
    EthBuiltPayload, EthPayloadBuilderAttributes, Events, PayloadBuilderHandle, PayloadId,
};
use reth_primitives::{Address, Bytes, ChainSpec, NodeRecord, B256};
use reth_tracing::tracing::info;
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio_stream::{
    wrappers::{BroadcastStream, UnboundedReceiverStream},
    StreamExt,
};

type TestProvider = BlockchainProvider<
    Arc<TempDatabase<DatabaseEnv>>,
    ShareableBlockchainTree<Arc<TempDatabase<DatabaseEnv>>, EvmProcessorFactory<EthEvmConfig>>,
>;

/// An helper struct to handle node actions
pub struct NodeHelper {
    engine_api_client: HttpClient,
    canonical_stream: CanonStateNotificationStream,

    payload_event_stream: BroadcastStream<Events<EthEngineTypes>>,
    payload_builder: PayloadBuilderHandle<EthEngineTypes>,

    network_events: UnboundedReceiverStream<NetworkEvent>,
    network: NetworkHandle,

    provider: TestProvider,

    node: TestNode,
}

impl NodeHelper {
    /// Creates a new test node
    pub async fn new(
        chain_spec: impl Into<Arc<ChainSpec>>,
        task_exec: TaskExecutor,
    ) -> eyre::Result<Self> {
        let network_config = NetworkArgs {
            discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
            ..NetworkArgs::default()
        };
        let node_config = NodeConfig::test()
            .with_chain(chain_spec)
            .with_network(network_config)
            .with_unused_ports()
            .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

        let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
            .testing_node(task_exec)
            .node(EthereumNode::default())
            .launch()
            .await?;

        let payload_events = node.payload_builder.subscribe().await?;
        let payload_event_stream = payload_events.into_stream();

        Ok(Self {
            network_events: node.network.event_listener(),
            network: node.network.clone(),
            payload_event_stream,
            payload_builder: node.payload_builder.clone(),
            engine_api_client: node.auth_server_handle().http_client(),
            canonical_stream: node.provider.canonical_state_stream(),
            provider: node.provider.clone(),
            node: node.clone(),
        })
    }

    /// Advances the node forward
    pub async fn advance(&mut self, raw_tx: Bytes) -> eyre::Result<(B256, B256)> {
        // push tx into pool via RPC server
        let tx_hash = self.inject_tx(raw_tx).await?;

        // trigger new payload building draining the pool
        let eth_attr = eth_payload_attributes();
        let payload_id = self.payload_builder.new_payload(eth_attr.clone()).await.unwrap();

        // first event is the payload attributes
        self.expect_attr_event(eth_attr.clone()).await?;

        // wait for the payload builder to have finished building
        self.wait_for_built_payload(payload_id).await;

        // trigger resolve payload via engine api
        let _ =
            EngineApiClient::<EthEngineTypes>::get_payload_v3(&self.engine_api_client, payload_id)
                .await?;

        // ensure we're also receiving the built payload as event
        let payload = self.expect_built_payload().await?;

        // submit payload via engine api
        let block_hash = self.submit_payload(payload, eth_attr.clone()).await?;

        // trigger forkchoice update via engine api to commit the block to the blockchain
        self.update_forkchoice(block_hash).await?;

        // assert the block has been committed to the blockchain
        self.assert_new_block(tx_hash, block_hash).await?;
        Ok((block_hash, tx_hash))
    }

    /// Injects a raw transaction into the node tx pool via RPC server
    async fn inject_tx(&mut self, raw_tx: Bytes) -> EthResult<B256> {
        let eth_api = self.node.rpc_registry.eth_api();
        eth_api.send_raw_transaction(raw_tx).await
    }

    /// Asserts the payload attribute event to be valid
    async fn expect_attr_event(
        &mut self,
        attributes: EthPayloadBuilderAttributes,
    ) -> eyre::Result<()> {
        let first_event = self.payload_event_stream.next().await.unwrap()?;
        if let reth::payload::Events::Attributes(attr) = first_event {
            assert_eq!(attributes.timestamp, attr.timestamp);
        } else {
            panic!("Expect first event as payload attributes.")
        }
        Ok(())
    }

    /// Waits until there is a non-empty payload built
    async fn wait_for_built_payload(&self, payload_id: PayloadId) {
        loop {
            let payload = self.payload_builder.best_payload(payload_id).await.unwrap().unwrap();
            if payload.block().body.is_empty() {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
            break;
        }
    }

    /// Expects a built payload event
    async fn expect_built_payload(&mut self) -> eyre::Result<EthBuiltPayload> {
        let second_event = self.payload_event_stream.next().await.unwrap()?;
        if let reth::payload::Events::BuiltPayload(payload) = second_event {
            Ok(payload)
        } else {
            panic!("Expect a built payload event.");
        }
    }

    /// Submits a payload to the engine api
    async fn submit_payload(
        &self,
        payload: EthBuiltPayload,
        eth_attr: EthPayloadBuilderAttributes,
    ) -> eyre::Result<B256> {
        // setup payload for submission
        let envelope_v3 = ExecutionPayloadEnvelopeV3::from(payload);
        let payload_v3 = envelope_v3.execution_payload;

        // submit payload to engine api
        let submission = EngineApiClient::<EthEngineTypes>::new_payload_v3(
            &self.engine_api_client,
            payload_v3,
            vec![],
            eth_attr.parent_beacon_block_root.unwrap(),
        )
        .await?;
        assert!(submission.is_valid());
        Ok(submission.latest_valid_hash.unwrap())
    }

    /// Asserts that a new block has been added to the blockchain
    /// and the tx has been included in the block
    pub async fn assert_new_block(
        &mut self,
        tip_tx_hash: B256,
        block_hash: B256,
    ) -> eyre::Result<()> {
        // get head block from notifications stream and verify the tx has been pushed to the
        // pool is actually present in the canonical block
        let head = self.canonical_stream.next().await.unwrap();
        let tx = head.tip().transactions().next();
        assert_eq!(tx.unwrap().hash().as_slice(), tip_tx_hash.as_slice());

        // wait for the block to commit
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // make sure the block hash we submitted via FCU engine api is the new latest block
        // using an RPC call
        let latest_block = self.provider.block_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap();
        assert_eq!(latest_block.hash_slow(), block_hash);
        Ok(())
    }

    /// Sends forkchoice update to the engine api
    pub async fn update_forkchoice(&self, hash: B256) -> eyre::Result<()> {
        EngineApiClient::<EthEngineTypes>::fork_choice_updated_v2(
            &self.engine_api_client,
            ForkchoiceState {
                head_block_hash: hash,
                safe_block_hash: hash,
                finalized_block_hash: hash,
            },
            None,
        )
        .await?;

        Ok(())
    }

    /// Adds a peer to the network node via network handle
    pub async fn add_peer(&mut self, node_record: NodeRecord) {
        self.network.peers_handle().add_peer(node_record.id, node_record.tcp_addr());

        match self.network_events.next().await {
            Some(NetworkEvent::PeerAdded(_)) => (),
            _ => panic!("Expected a peer added event"),
        }
    }

    /// Returns the network node record
    pub fn record(&self) -> NodeRecord {
        self.network.local_node_record()
    }

    /// Expects a new p2p session to be established
    pub async fn expect_session(&mut self) {
        match self.network_events.next().await {
            Some(NetworkEvent::SessionEstablished { remote_addr, .. }) => {
                info!(?remote_addr, "Session established")
            }
            _ => panic!("Expected session established event"),
        }
    }
}

/// Helper function to create a new eth payload attributes
pub fn eth_payload_attributes() -> EthPayloadBuilderAttributes {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    let attributes = PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };
    EthPayloadBuilderAttributes::new(B256::ZERO, attributes)
}

type TestNode = FullNode<
    FullNodeComponentsAdapter<
        FullNodeTypesAdapter<
            EthereumNode,
            Arc<TempDatabase<DatabaseEnv>>,
            BlockchainProvider<
                Arc<TempDatabase<DatabaseEnv>>,
                ShareableBlockchainTree<
                    Arc<TempDatabase<DatabaseEnv>>,
                    EvmProcessorFactory<EthEvmConfig>,
                >,
            >,
        >,
        Pool<
            TransactionValidationTaskExecutor<
                EthTransactionValidator<
                    BlockchainProvider<
                        Arc<TempDatabase<DatabaseEnv>>,
                        ShareableBlockchainTree<
                            Arc<TempDatabase<DatabaseEnv>>,
                            EvmProcessorFactory<EthEvmConfig>,
                        >,
                    >,
                    EthPooledTransaction,
                >,
            >,
            CoinbaseTipOrdering<EthPooledTransaction>,
            DiskFileBlobStore,
        >,
    >,
>;
