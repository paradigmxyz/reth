use reth::{
    args::{DiscoveryArgs, NetworkArgs, RpcServerArgs},
    rpc::types::engine::PayloadAttributes,
    tasks::{TaskExecutor, TaskManager},
};
use reth_e2e_test_utils::{node::NodeHelper, wallet::Wallet};
use reth_node_builder::{NodeBuilder, NodeConfig, NodeHandle};
use reth_node_optimism::{OptimismNode, OptimismPayloadBuilderAttributes};
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_primitives::{Address, BlockHash, ChainSpecBuilder, Genesis, B256, BASE_MAINNET};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) fn setup() -> (NodeConfig, TaskManager, TaskExecutor, Wallet) {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(BASE_MAINNET.chain)
            .genesis(genesis)
            .ecotone_activated()
            .build(),
    );
    let chain_id = chain_spec.chain.into();

    let network_config = NetworkArgs {
        discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
        ..NetworkArgs::default()
    };

    (
        NodeConfig::test()
            .with_chain(chain_spec)
            .with_network(network_config)
            .with_unused_ports()
            .with_rpc(RpcServerArgs::default().with_unused_ports().with_http()),
        tasks,
        exec,
        Wallet::default().with_chain_id(chain_id),
    )
}

pub(crate) async fn node(
    node_config: NodeConfig,
    exec: TaskExecutor,
    id: usize,
) -> eyre::Result<OpNode> {
    let span = span!(Level::INFO, "node", id);
    let _enter = span.enter();
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(exec.clone())
        .node(OptimismNode::default())
        .launch()
        .await?;

    NodeHelper::new(node).await
}

pub(crate) async fn advance_chain(
    length: usize,
    node: &mut OpNode,
    wallet: Arc<Mutex<Wallet>>,
) -> eyre::Result<Vec<BlockHash>> {
    node.advance(
        length as u64,
        || {
            let wallet = wallet.clone();
            Box::pin(async move { wallet.lock().await.optimism_l1_block_info_tx().await })
        },
        optimism_payload_attributes,
    )
    .await
}

/// Helper function to create a new eth payload attributes
pub(crate) fn optimism_payload_attributes(timestamp: u64) -> OptimismPayloadBuilderAttributes {
    let attributes = PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };

    OptimismPayloadBuilderAttributes {
        payload_attributes: EthPayloadBuilderAttributes::new(B256::ZERO, attributes),
        transactions: vec![],
        no_tx_pool: false,
        gas_limit: Some(30_000_000),
    }
}

// Type alias
type OpNode = NodeHelper<
    reth_node_api::FullNodeComponentsAdapter<
        reth_node_api::FullNodeTypesAdapter<
            OptimismNode,
            Arc<reth_db::test_utils::TempDatabase<reth_db::DatabaseEnv>>,
            reth_provider::providers::BlockchainProvider<
                Arc<reth_db::test_utils::TempDatabase<reth_db::DatabaseEnv>>,
                reth::blockchain_tree::ShareableBlockchainTree<
                    Arc<reth_db::test_utils::TempDatabase<reth_db::DatabaseEnv>>,
                    reth_revm::EvmProcessorFactory<reth_node_optimism::OptimismEvmConfig>,
                >,
            >,
        >,
        reth_transaction_pool::Pool<
            reth_transaction_pool::TransactionValidationTaskExecutor<
                reth_node_optimism::txpool::OpTransactionValidator<
                    reth_provider::providers::BlockchainProvider<
                        Arc<reth_db::test_utils::TempDatabase<reth_db::DatabaseEnv>>,
                        reth::blockchain_tree::ShareableBlockchainTree<
                            Arc<reth_db::test_utils::TempDatabase<reth_db::DatabaseEnv>>,
                            reth_revm::EvmProcessorFactory<reth_node_optimism::OptimismEvmConfig>,
                        >,
                    >,
                    reth_transaction_pool::EthPooledTransaction,
                >,
            >,
            reth_transaction_pool::CoinbaseTipOrdering<reth_transaction_pool::EthPooledTransaction>,
            reth_transaction_pool::blobstore::DiskFileBlobStore,
        >,
    >,
>;
