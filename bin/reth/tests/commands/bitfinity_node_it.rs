//!
//! Integration tests for the bitfinity node command.

use super::utils::*;
use did::keccak;
use eth_server::{EthImpl, EthServer};
use ethereum_json_rpc_client::{reqwest::ReqwestClient, CertifiedResult, EthJsonRpcClient};
use jsonrpsee::{
    server::{Server, ServerHandle},
    Methods, RpcModule,
};
use rand::RngCore;
use reth::{
    args::{DatadirArgs, RpcServerArgs},
    dirs::{DataDirPath, MaybePlatformPath},
};
use reth_consensus::FullConsensus;
use reth_db::test_utils::TempDatabase;
use reth_db::DatabaseEnv;
use reth_db::{init_db, test_utils::tempdir_path};
use reth_discv5::discv5::enr::secp256k1::{Keypair, Secp256k1};
use reth_network::NetworkHandle;
use reth_node_api::{FullNodeTypesAdapter, NodeTypesWithDBAdapter};
use reth_node_builder::components::Components;
use reth_node_builder::engine_tree_config::TreeConfig;
use reth_node_builder::rpc::RpcAddOns;
use reth_node_builder::{EngineNodeLauncher, NodeAdapter, NodeBuilder, NodeConfig, NodeHandle};
use reth_node_ethereum::node::{EthereumAddOns, EthereumEngineValidatorBuilder};
use reth_node_ethereum::{
    BasicBlockExecutorProvider, EthEvmConfig, EthExecutionStrategyFactory, EthereumNode,
};
use reth_primitives::{Transaction, TransactionSigned};
use reth_provider::providers::BlockchainProvider;
use reth_rpc::EthApi;
use reth_tasks::TaskManager;
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, CoinbaseTipOrdering, EthPooledTransaction,
    EthTransactionValidator, Pool, TransactionValidationTaskExecutor,
};
use revm_primitives::{hex, Address, B256, U256};
use std::{net::SocketAddr, str::FromStr, sync::Arc};

#[tokio::test]
async fn bitfinity_test_should_start_local_reth_node() {
    // Arrange
    let _log = init_logs();
    let (reth_client, _reth_node, _tasks) = start_reth_node(None, None).await;

    // Act & Assert
    assert!(reth_client.get_chain_id().await.is_ok());
}

#[tokio::test]
async fn bitfinity_test_lb_lag_check() {
    // Arrange
    let _log = init_logs();

    let eth_server = EthImpl::new();
    let (_server, eth_server_address) =
        mock_eth_server_start(EthServer::into_rpc(eth_server)).await;
    let (reth_client, _reth_node, _tasks) =
        start_reth_node(Some(format!("http://{}", eth_server_address)), None).await;

    // Try `eth_lbLagCheck`
    let result: String = reth_client
        .single_request(
            "eth_lbLagCheck".to_owned(),
            ethereum_json_rpc_client::Params::Array(vec![10.into()]),
            ethereum_json_rpc_client::Id::Num(1),
        )
        .await
        .unwrap();

    assert!(result.contains("ACCEPTABLE_LAG"), "{result:?}");

    // Need time to generate extra blocks at `eth_server`
    // Assuming `EthImpl` ticks 100ms for the each next block
    let mut lag_check_ok = false;
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(50));
    for _ in 0..100 {
        interval.tick().await;

        let result = reth_client
            .single_request::<String>(
                "eth_lbLagCheck".to_owned(),
                ethereum_json_rpc_client::Params::Array(vec![5.into()]),
                ethereum_json_rpc_client::Id::Num(1),
            )
            .await;
        if let Ok(message) = result {
            if message.contains("LAGGING") {
                lag_check_ok = true;
                break;
            }
        }
    }

    assert!(lag_check_ok);

    // And should not lag with bigger acceptable delta
    let result: String = reth_client
        .single_request(
            "eth_lbLagCheck".to_owned(),
            ethereum_json_rpc_client::Params::Array(vec![1000.into()]),
            ethereum_json_rpc_client::Id::Num(1),
        )
        .await
        .unwrap();

    assert!(result.contains("ACCEPTABLE_LAG"), "{result:?}");
}

#[tokio::test]
async fn bitfinity_test_lb_lag_check_fail_safe() {
    let (reth_client, _reth_node, _tasks) =
        start_reth_node(Some("http://local_host:11".to_string()), None).await;

    let message: String = reth_client
        .single_request(
            "eth_lbLagCheck".to_owned(),
            ethereum_json_rpc_client::Params::Array(vec![1000.into()]),
            ethereum_json_rpc_client::Id::Num(1),
        )
        .await
        .unwrap();

    // Response should be OK to do not break LB if source temporary not available
    assert!(message.contains("ACCEPTABLE_LAG"), "{message}");
    assert!(message.contains("NO_SOURCE"), "{message}");
}

#[tokio::test]
async fn bitfinity_test_node_forward_ic_or_eth_get_last_certified_block() {
    // Arrange
    let _log = init_logs();

    let eth_server = EthImpl::new();
    let (_server, eth_server_address) =
        mock_eth_server_start(EthServer::into_rpc(eth_server)).await;
    let (reth_client, _reth_node, _tasks) =
        start_reth_node(Some(format!("http://{}", eth_server_address)), None).await;

    // Act
    let result = reth_client.get_last_certified_block().await;

    // Assert
    assert!(result.is_ok());

    // Try with `eth_getLastCertifiedBlock` alias
    let result: CertifiedResult<did::Block<did::H256>> = reth_client
        .single_request(
            "eth_getLastCertifiedBlock".to_owned(),
            ethereum_json_rpc_client::Params::None,
            ethereum_json_rpc_client::Id::Num(1),
        )
        .await
        .unwrap();

    assert_eq!(result.certificate, vec![1u8, 3, 11]);
}

#[tokio::test]
async fn bitfinity_test_node_forward_get_gas_price_requests() {
    // Arrange
    let _log = init_logs();

    let eth_server = EthImpl::new();
    let gas_price = eth_server.gas_price;
    let (_server, eth_server_address) =
        mock_eth_server_start(EthServer::into_rpc(eth_server)).await;
    let (reth_client, _reth_node, _tasks) =
        start_reth_node(Some(format!("http://{}", eth_server_address)), None).await;

    // Act
    let gas_price_result = reth_client.gas_price().await;

    // Assert
    assert_eq!(gas_price_result.unwrap(), gas_price.into());
}

#[tokio::test]
async fn bitfinity_test_node_forward_max_priority_fee_per_gas_requests() {
    // Arrange
    let _log = init_logs();

    let eth_server = EthImpl::new();
    let max_priority_fee_per_gas = eth_server.max_priority_fee_per_gas;
    let (_server, eth_server_address) =
        mock_eth_server_start(EthServer::into_rpc(eth_server)).await;
    let (reth_client, _reth_node, _tasks) =
        start_reth_node(Some(format!("http://{}", eth_server_address)), None).await;

    // Act
    let result = reth_client.max_priority_fee_per_gas().await;

    // Assert
    assert_eq!(result.unwrap(), max_priority_fee_per_gas.into());
}

#[tokio::test]
async fn bitfinity_test_node_forward_eth_get_genesis_balances() {
    // Arrange
    let _log = init_logs();

    let eth_server = EthImpl::new().with_genesis_balances(vec![
        (Address::from_slice(&[1u8; 20]), U256::from(10)),
        (Address::from_slice(&[2u8; 20]), U256::from(20)),
        (Address::from_slice(&[3u8; 20]), U256::from(30)),
    ]);
    let (_server, eth_server_address) =
        mock_eth_server_start(EthServer::into_rpc(eth_server)).await;
    let (reth_client, _reth_node, _tasks) =
        start_reth_node(Some(format!("http://{}", eth_server_address)), None).await;

    // Act
    let result: Vec<(did::H160, did::U256)> = reth_client
        .single_request(
            "eth_getGenesisBalances".to_owned(),
            ethereum_json_rpc_client::Params::None,
            ethereum_json_rpc_client::Id::Num(1),
        )
        .await
        .unwrap();

    // Assert
    assert_eq!(result.len(), 3);

    assert_eq!(result[0].0, Address::from_slice(&[1u8; 20]).into());
    assert_eq!(result[0].1, U256::from(10).into());

    assert_eq!(result[1].0, Address::from_slice(&[2u8; 20]).into());
    assert_eq!(result[1].1, U256::from(20).into());

    assert_eq!(result[2].0, Address::from_slice(&[3u8; 20]).into());
    assert_eq!(result[2].1, U256::from(30).into());
}

#[tokio::test]
async fn bitfinity_test_node_forward_ic_get_genesis_balances() {
    // Arrange
    let _log = init_logs();

    let eth_server = EthImpl::new().with_genesis_balances(vec![
        (Address::from_slice(&[1u8; 20]), U256::from(10)),
        (Address::from_slice(&[2u8; 20]), U256::from(20)),
        (Address::from_slice(&[3u8; 20]), U256::from(30)),
    ]);
    let (_server, eth_server_address) =
        mock_eth_server_start(EthServer::into_rpc(eth_server)).await;
    let (reth_client, _reth_node, _tasks) =
        start_reth_node(Some(format!("http://{}", eth_server_address)), None).await;

    // Act
    let result = reth_client.get_genesis_balances().await.unwrap();

    // Assert
    assert_eq!(result.len(), 3);

    assert_eq!(result[0].0, Address::from_slice(&[1u8; 20]).into());
    assert_eq!(result[0].1, U256::from(10).into());

    assert_eq!(result[1].0, Address::from_slice(&[2u8; 20]).into());
    assert_eq!(result[1].1, U256::from(20).into());

    assert_eq!(result[2].0, Address::from_slice(&[3u8; 20]).into());
    assert_eq!(result[2].1, U256::from(30).into());
}

#[tokio::test]
async fn bitfinity_test_node_forward_send_raw_transaction_requests() {
    // Arrange
    let _log = init_logs();

    let eth_server = EthImpl::new();
    let (_server, eth_server_address) =
        mock_eth_server_start(EthServer::into_rpc(eth_server)).await;
    let (reth_client, _reth_node, _tasks) =
        start_reth_node(Some(format!("http://{}", eth_server_address)), None).await;

    // Create a random transaction
    let mut tx = [0u8; 256];
    rand::thread_rng().fill_bytes(&mut tx);
    let expected_tx_hash = keccak::keccak_hash(format!("0x{}", hex::encode(tx)).as_bytes());

    // Act
    let result = reth_client.send_raw_transaction_bytes(&tx).await;

    // Assert
    assert_eq!(result.unwrap(), expected_tx_hash);
}

#[tokio::test]
async fn bitfinity_test_node_forward_get_transaction_by_hash() {
    // Arrange
    let _log = init_logs();

    let eth_server = EthImpl::new();
    let last_get_tx_hash = eth_server.last_get_tx_by_hash.clone();

    let (_server, eth_server_address) =
        mock_eth_server_start(EthServer::into_rpc(eth_server)).await;
    let (reth_client, _reth_node, _tasks) =
        start_reth_node(Some(format!("http://{}", eth_server_address)), None).await;

    let hash = B256::random();

    // Act
    let tx_result = reth_client.get_transaction_by_hash(hash.into()).await;

    // Assert
    assert!(matches!(tx_result, Ok(Some(_))));
    assert_eq!(last_get_tx_hash.read().await.unwrap(), hash);
}

fn sign_tx_with_random_key_pair(tx: Transaction) -> TransactionSigned {
    let secp = Secp256k1::new();
    let key_pair = Keypair::new(&secp, &mut rand::thread_rng());
    sign_tx_with_key_pair(key_pair, tx)
}

fn sign_tx_with_key_pair(key_pair: Keypair, tx: Transaction) -> TransactionSigned {
    let signature = reth_primitives::sign_message(
        B256::from_slice(&key_pair.secret_bytes()[..]),
        tx.signature_hash(),
    )
    .unwrap();
    TransactionSigned::new(tx, signature, Default::default())
}

/// Start a local reth node
pub async fn start_reth_node(
    bitfinity_evm_url: Option<String>,
    import_data: Option<ImportData>,
) -> (
    EthJsonRpcClient<ReqwestClient>,
    NodeHandle<
        NodeAdapter<
            FullNodeTypesAdapter<
                EthereumNode,
                Arc<TempDatabase<DatabaseEnv>>,
                BlockchainProvider<
                    NodeTypesWithDBAdapter<EthereumNode, Arc<TempDatabase<DatabaseEnv>>>,
                >,
            >,
            Components<
                FullNodeTypesAdapter<
                    EthereumNode,
                    Arc<TempDatabase<DatabaseEnv>>,
                    BlockchainProvider<
                        NodeTypesWithDBAdapter<EthereumNode, Arc<TempDatabase<DatabaseEnv>>>,
                    >,
                >,
                reth_network::EthNetworkPrimitives,
                Pool<
                    TransactionValidationTaskExecutor<
                        EthTransactionValidator<
                            BlockchainProvider<
                                NodeTypesWithDBAdapter<
                                    EthereumNode,
                                    Arc<TempDatabase<DatabaseEnv>>,
                                >,
                            >,
                            EthPooledTransaction,
                        >,
                    >,
                    CoinbaseTipOrdering<EthPooledTransaction>,
                    DiskFileBlobStore,
                >,
                EthEvmConfig,
                BasicBlockExecutorProvider<EthExecutionStrategyFactory>,
                Arc<dyn FullConsensus>,
            >,
        >,
        RpcAddOns<
            NodeAdapter<
                FullNodeTypesAdapter<
                    EthereumNode,
                    Arc<TempDatabase<DatabaseEnv>>,
                    BlockchainProvider<
                        NodeTypesWithDBAdapter<EthereumNode, Arc<TempDatabase<DatabaseEnv>>>,
                    >,
                >,
                Components<
                    FullNodeTypesAdapter<
                        EthereumNode,
                        Arc<TempDatabase<DatabaseEnv>>,
                        BlockchainProvider<
                            NodeTypesWithDBAdapter<EthereumNode, Arc<TempDatabase<DatabaseEnv>>>,
                        >,
                    >,
                    reth_network::EthNetworkPrimitives,
                    Pool<
                        TransactionValidationTaskExecutor<
                            EthTransactionValidator<
                                BlockchainProvider<
                                    NodeTypesWithDBAdapter<
                                        EthereumNode,
                                        Arc<TempDatabase<DatabaseEnv>>,
                                    >,
                                >,
                                EthPooledTransaction,
                            >,
                        >,
                        CoinbaseTipOrdering<EthPooledTransaction>,
                        DiskFileBlobStore,
                    >,
                    EthEvmConfig,
                    BasicBlockExecutorProvider<EthExecutionStrategyFactory>,
                    Arc<dyn FullConsensus>,
                >,
            >,
            EthApi<
                BlockchainProvider<
                    NodeTypesWithDBAdapter<EthereumNode, Arc<TempDatabase<DatabaseEnv>>>,
                >,
                Pool<
                    TransactionValidationTaskExecutor<
                        EthTransactionValidator<
                            BlockchainProvider<
                                NodeTypesWithDBAdapter<
                                    EthereumNode,
                                    Arc<TempDatabase<DatabaseEnv>>,
                                >,
                            >,
                            EthPooledTransaction,
                        >,
                    >,
                    CoinbaseTipOrdering<EthPooledTransaction>,
                    DiskFileBlobStore,
                >,
                NetworkHandle,
                EthEvmConfig,
            >,
            EthereumEngineValidatorBuilder,
        >,
    >,
    TaskManager,
) {
    let tasks = TaskManager::current();

    // create node config
    let mut node_config =
        NodeConfig::test().dev().with_rpc(RpcServerArgs::default().with_http()).with_unused_ports();
    node_config.dev.dev = false;

    let mut chain = node_config.chain.as_ref().clone();
    chain.bitfinity_evm_url = bitfinity_evm_url;
    let mut node_config = node_config.with_chain(chain);

    let database = if let Some(import_data) = import_data {
        let data_dir = MaybePlatformPath::<DataDirPath>::from_str(
            import_data.data_dir.data_dir().to_str().unwrap(),
        )
        .unwrap();
        let mut data_dir_args = node_config.datadir.clone();
        data_dir_args.datadir = data_dir;
        data_dir_args.static_files_path = Some(import_data.data_dir.static_files());
        node_config = node_config.with_datadir_args(data_dir_args);
        node_config = node_config.with_chain(import_data.chain.clone());
        import_data.database
    } else {
        let path = MaybePlatformPath::<DataDirPath>::from(tempdir_path());
        node_config = node_config
            .with_datadir_args(DatadirArgs { datadir: path.clone(), ..Default::default() });
        let data_dir =
            path.unwrap_or_chain_default(node_config.chain.chain, node_config.datadir.clone());
        Arc::new(init_db(data_dir.db(), Default::default()).unwrap())
    };

    let exec = tasks.executor();
    let node_handle = NodeBuilder::new(node_config)
        .with_database(database)
        .testing_node(exec)
        .with_types_and_provider::<EthereumNode, BlockchainProvider<_>>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .launch_with_fn(|builder| {
            let launcher = EngineNodeLauncher::new(
                builder.task_executor().clone(),
                builder.config().datadir(),
                TreeConfig::default(),
            );
            builder.launch_with(launcher)
        })
        .await
        .unwrap();

    let reth_address = node_handle.node.rpc_server_handle().http_local_addr().unwrap();

    let client: EthJsonRpcClient<ReqwestClient> =
        EthJsonRpcClient::new(ReqwestClient::new(format!("http://{}", reth_address)));

    (client, node_handle, tasks)
}

/// Start a local Eth server.
/// Reth requests will be forwarded to this server
pub async fn mock_eth_server_start(methods: impl Into<Methods>) -> (ServerHandle, SocketAddr) {
    mock_multi_server_start([methods.into()]).await
}

/// Starts a local mock server that combines methods from different sources.
pub async fn mock_multi_server_start(
    methods: impl IntoIterator<Item = Methods>,
) -> (ServerHandle, SocketAddr) {
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let server = Server::builder().build(addr).await.unwrap();

    let mut module = RpcModule::new(());

    for method_group in methods {
        module.merge(method_group).unwrap();
    }

    let server_address = server.local_addr().unwrap();
    let handle = server.start(module);

    (handle, server_address)
}

/// Eth server mock for local testing
pub mod eth_server {

    use std::sync::{atomic::Ordering, Arc};

    use alloy_consensus::constants::{EMPTY_RECEIPTS, EMPTY_TRANSACTIONS};
    use alloy_rlp::Bytes;
    use did::{keccak, BlockConfirmationData, BlockConfirmationResult, BlockNumber, H256, U64};
    use ethereum_json_rpc_client::CertifiedResult;
    use jsonrpsee::{core::RpcResult, proc_macros::rpc};
    use reth_transaction_pool::test_utils::MockTransaction;

    use reth_trie::EMPTY_ROOT_HASH;
    use revm_primitives::{Address, B256, U256};
    use tokio::sync::RwLock;

    use super::sign_tx_with_random_key_pair;

    #[rpc(server, namespace = "eth")]
    pub trait Eth {
        /// Returns the current gas price.
        #[method(name = "gasPrice")]
        async fn gas_price(&self) -> RpcResult<U256>;

        /// Returns the current max priority fee per gas.
        #[method(name = "maxPriorityFeePerGas")]
        async fn max_priority_fee_per_gas(&self) -> RpcResult<U256>;

        /// Sends a raw transaction.
        #[method(name = "sendRawTransaction")]
        async fn send_raw_transaction(&self, tx: Bytes) -> RpcResult<B256>;

        /// Gets transaction by hash.
        #[method(name = "getTransactionByHash")]
        async fn transaction_by_hash(&self, hash: B256) -> RpcResult<Option<did::Transaction>>;

        /// Returns the genesis balances.
        #[method(name = "getGenesisBalances", aliases = ["ic_getGenesisBalances"])]
        async fn get_genesis_balances(&self) -> RpcResult<Vec<(Address, U256)>>;

        /// Returns the last certified block.
        #[method(name = "getLastCertifiedBlock", aliases = ["ic_getLastCertifiedBlock"])]
        async fn get_last_certified_block(
            &self,
        ) -> RpcResult<CertifiedResult<did::Block<did::H256>>>;

        /// Returns a block by block number
        #[method(name = "getBlockByNumber")]
        async fn get_block_by_number(
            &self,
            number: BlockNumber,
        ) -> RpcResult<Option<did::Block<did::H256>>>;

        /// Returns the chain ID
        #[method(name = "chainId")]
        async fn chain_id(&self) -> RpcResult<U256>;

        /// Returns the last block number
        #[method(name = "blockNumber")]
        async fn block_number(&self) -> RpcResult<U256>;

        /// Returns the EVM global state
        #[method(name = "getEvmGlobalState", aliases = ["ic_getEvmGlobalState"])]
        async fn get_evm_global_state(&self) -> RpcResult<did::evm_state::EvmGlobalState>;
    }

    #[rpc(server, namespace = "ic")]
    trait BfEvm {
        /// Send block confirmation request.
        #[method(name = "sendConfirmBlock")]
        async fn confirm_block(
            &self,
            data: BlockConfirmationData,
        ) -> RpcResult<BlockConfirmationResult>;
    }

    /// Eth server implementation for local testing
    #[derive(Debug)]
    pub struct EthImpl {
        /// Current gas price
        pub gas_price: u128,
        /// Current max priority fee per gas
        pub max_priority_fee_per_gas: u128,

        /// Hash of last
        pub last_get_tx_by_hash: Arc<RwLock<Option<B256>>>,
        /// Current block number (atomic for thread safety)
        pub current_block: Arc<std::sync::atomic::AtomicU64>,
        /// Chain ID
        pub chain_id: u64,
        /// EVM staging mode
        pub state: did::evm_state::EvmGlobalState,
        /// Block production task handle
        #[allow(dead_code)]
        block_task: Option<tokio::task::JoinHandle<()>>,
        /// Genesis Balances
        pub genesis_balances: Vec<(Address, U256)>,
        /// Unsafe blocks count
        pub unsafe_blocks_count: u64,
        /// Custom state root hash
        pub custom_state_root: H256,
    }

    impl EthImpl {
        /// Create a new Eth server implementation
        pub fn new() -> Self {
            Self::new_with_max_block(u64::MAX)
        }

        /// Creates a new Eth server implementation that will mint blocks until `max_block` number
        pub fn new_with_max_block(max_block: u64) -> Self {
            // Fake block counter
            let current_block = Arc::new(std::sync::atomic::AtomicU64::new(1));
            let block_counter = current_block.clone();

            let block_task = Some(tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
                loop {
                    interval.tick().await;
                    let block = block_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::info!("Minted block {}", block + 1);

                    if block + 1 >= max_block {
                        break;
                    }
                }
            }));

            Self {
                gas_price: rand::random(),
                max_priority_fee_per_gas: rand::random(),
                last_get_tx_by_hash: Default::default(),
                current_block,
                chain_id: 1,
                state: did::evm_state::EvmGlobalState::Staging { max_block_number: None },
                block_task,
                genesis_balances: vec![],
                unsafe_blocks_count: 0,
                custom_state_root: EMPTY_ROOT_HASH.into(),
            }
        }

        /// Create a new instance with custom state
        pub fn with_evm_state(state: did::evm_state::EvmGlobalState) -> Self {
            let mut instance = Self::new();
            instance.state = state;
            instance
        }

        /// Set the genesis balances
        pub fn with_genesis_balances(mut self, balances: Vec<(Address, U256)>) -> Self {
            self.genesis_balances = balances;
            self
        }

        /// Set a custom state root hash
        pub fn with_state_root(mut self, state_root: did::H256) -> Self {
            self.custom_state_root = state_root;
            self
        }

        /// Returns an implementation of Bitfinity EVM canister API
        pub fn bf_impl(&mut self, unsafe_blocks_count: u64) -> BfEvmImpl {
            self.unsafe_blocks_count = unsafe_blocks_count;
            BfEvmImpl { confirm_until: u64::MAX }
        }
    }

    impl Drop for EthImpl {
        fn drop(&mut self) {
            // Abort the background task when the EthImpl is dropped
            if let Some(task) = self.block_task.take() {
                task.abort();
            }
        }
    }

    impl Default for EthImpl {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait::async_trait]
    impl EthServer for EthImpl {
        async fn get_block_by_number(
            &self,
            number: BlockNumber,
        ) -> RpcResult<Option<did::Block<did::H256>>> {
            let current_block = self.current_block.load(Ordering::Relaxed);
            let block_num = match number {
                BlockNumber::Safe => current_block - self.unsafe_blocks_count,
                BlockNumber::Latest | BlockNumber::Finalized => current_block,
                BlockNumber::Earliest => 0,
                BlockNumber::Pending => {
                    self.current_block.load(std::sync::atomic::Ordering::Relaxed) + 1
                }
                BlockNumber::Number(num) => num.as_u64(),
            };

            if block_num > current_block {
                return Ok(None);
            }

            let mut block = did::Block {
                number: block_num.into(),
                timestamp: did::U256::from(1234567890_u64 + block_num),
                gas_limit: did::U256::from(30_000_000_u64),
                base_fee_per_gas: Some(did::U256::from(7_u64)),
                state_root: self.custom_state_root.clone(),
                receipts_root: EMPTY_RECEIPTS.into(),
                transactions_root: EMPTY_TRANSACTIONS.into(),
                parent_hash: if block_num == 0 {
                    H256::zero()
                } else {
                    self.get_block_by_number(BlockNumber::Number(U64::from(block_num - 1)))
                        .await?
                        .unwrap()
                        .hash
                },
                ..Default::default()
            };

            block.hash = keccak::keccak_hash(&block.header_rlp_encoded());
            Ok(Some(block))
        }

        async fn chain_id(&self) -> RpcResult<U256> {
            Ok(U256::from(self.chain_id))
        }

        async fn block_number(&self) -> RpcResult<U256> {
            Ok(U256::from(self.current_block.load(std::sync::atomic::Ordering::Relaxed)))
        }

        async fn get_evm_global_state(&self) -> RpcResult<did::evm_state::EvmGlobalState> {
            Ok(self.state.clone())
        }

        async fn gas_price(&self) -> RpcResult<U256> {
            Ok(U256::from(self.gas_price))
        }

        async fn max_priority_fee_per_gas(&self) -> RpcResult<U256> {
            Ok(U256::from(self.max_priority_fee_per_gas))
        }

        async fn send_raw_transaction(&self, tx: Bytes) -> RpcResult<B256> {
            let hash = keccak::keccak_hash(&tx);
            Ok(hash.into())
        }

        async fn transaction_by_hash(&self, hash: B256) -> RpcResult<Option<did::Transaction>> {
            *self.last_get_tx_by_hash.write().await = Some(hash);

            // return transaction, initialized enough to pass reth checks.
            let tx = sign_tx_with_random_key_pair(MockTransaction::legacy().with_hash(hash).into());
            let did_tx = did::Transaction {
                hash: hash.into(),
                r: tx.signature.r().into(),
                s: tx.signature.s().into(),
                v: (tx.signature.recid().to_byte() as u64).into(),
                ..Default::default()
            };

            Ok(Some(did_tx))
        }

        async fn get_genesis_balances(&self) -> RpcResult<Vec<(Address, U256)>> {
            Ok(self.genesis_balances.clone())
        }

        async fn get_last_certified_block(
            &self,
        ) -> RpcResult<CertifiedResult<did::Block<did::H256>>> {
            Ok(CertifiedResult {
                data: did::Block {
                    number: 20_u64.into(),
                    hash: H256::from_slice(&[20; 32]),
                    parent_hash: H256::from_slice(&[19; 32]),
                    timestamp: did::U256::from(1234567890_u64 + 20),
                    state_root: H256::from_slice(&[20; 32]),
                    transactions: vec![],
                    ..Default::default()
                },
                witness: vec![],
                certificate: vec![1u8, 3, 11],
            })
        }
    }

    /// Mock implementation of Bitfinity EVM canister API
    #[derive(Debug)]
    pub struct BfEvmImpl {
        /// Will allow confirming block until this block number
        pub confirm_until: u64,
    }

    #[async_trait::async_trait]
    impl BfEvmServer for BfEvmImpl {
        async fn confirm_block(
            &self,
            data: BlockConfirmationData,
        ) -> RpcResult<BlockConfirmationResult> {
            if data.block_number > self.confirm_until {
                Ok(BlockConfirmationResult::NotConfirmed)
            } else {
                Ok(BlockConfirmationResult::Confirmed)
            }
        }
    }
}
