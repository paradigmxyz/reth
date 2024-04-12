use crate::{engine_api::EngineApiHelper, network::NetworkHelper, payload::PayloadHelper};
use alloy_rpc_types::BlockNumberOrTag;
use eyre::Ok;
use reth::{
    blockchain_tree::ShareableBlockchainTree,
    builder::{
        components::FullNodeComponentsAdapter, FullNode, FullNodeTypesAdapter, NodeBuilder,
        NodeHandle,
    },
    providers::{providers::BlockchainProvider, BlockReaderIdExt, CanonStateSubscriptions},
    revm::EvmProcessorFactory,
    rpc::{
        eth::{error::EthResult, EthTransactions},
        types::engine::PayloadAttributes,
    },
    tasks::TaskExecutor,
    transaction_pool::{
        blobstore::DiskFileBlobStore, CoinbaseTipOrdering, EthPooledTransaction,
        EthTransactionValidator, Pool, TransactionValidationTaskExecutor,
    },
};
use reth_db::{test_utils::TempDatabase, DatabaseEnv};
use reth_node_core::node_config::NodeConfig;
use reth_node_ethereum::{EthEvmConfig, EthereumNode};
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_primitives::{Address, Bytes, B256};

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio_stream::StreamExt;

/// An helper struct to handle node actions
pub struct NodeHelper {
    pub inner: TestNode,
    payload: PayloadHelper,
    pub network: NetworkHelper,
    pub engine_api: EngineApiHelper,
}

impl NodeHelper {
    /// Creates a new test node
    pub async fn new(node_config: NodeConfig, task_exec: TaskExecutor) -> eyre::Result<Self> {
        let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
            .testing_node(task_exec)
            .node(EthereumNode::default())
            .launch()
            .await?;

        Ok(Self {
            inner: node.clone(),
            network: NetworkHelper::new(node.network.clone()),
            payload: PayloadHelper::new(node.payload_builder.clone()).await?,
            engine_api: EngineApiHelper {
                engine_api_client: node.auth_server_handle().http_client(),
                canonical_stream: node.provider.canonical_state_stream(),
            },
        })
    }

    /// Advances the node forward
    pub async fn advance(&mut self, raw_tx: Bytes) -> eyre::Result<(B256, B256)> {
        // push tx into pool via RPC server
        let tx_hash = self.inject_tx(raw_tx).await?;

        // trigger new payload building draining the pool
        let eth_attr = self.payload.new_payload().await.unwrap();

        // first event is the payload attributes
        self.payload.expect_attr_event(eth_attr.clone()).await?;

        // wait for the payload builder to have finished building
        self.payload.wait_for_built_payload(eth_attr.payload_id()).await;

        // trigger resolve payload via engine api
        self.engine_api.get_payload_v3(eth_attr.payload_id()).await?;

        // ensure we're also receiving the built payload as event
        let payload = self.payload.expect_built_payload().await?;

        // submit payload via engine api
        let block_hash = self.engine_api.submit_payload(payload, eth_attr.clone()).await?;

        // trigger forkchoice update via engine api to commit the block to the blockchain
        self.engine_api.update_forkchoice(block_hash).await?;

        // assert the block has been committed to the blockchain
        self.assert_new_block(tx_hash, block_hash).await?;
        Ok((block_hash, tx_hash))
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
    ) -> eyre::Result<()> {
        // get head block from notifications stream and verify the tx has been pushed to the
        // pool is actually present in the canonical block
        let head = self.engine_api.canonical_stream.next().await.unwrap();
        let tx = head.tip().transactions().next();
        assert_eq!(tx.unwrap().hash().as_slice(), tip_tx_hash.as_slice());

        // wait for the block to commit
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // make sure the block hash we submitted via FCU engine api is the new latest block
        // using an RPC call
        let latest_block =
            self.inner.provider.block_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap();
        assert_eq!(latest_block.hash_slow(), block_hash);
        Ok(())
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
