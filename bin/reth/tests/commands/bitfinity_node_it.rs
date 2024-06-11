//!
//! Integration tests for the bitfinity node command.
//!

use super::utils::*;
use ethereum_json_rpc_client::{reqwest::ReqwestClient, EthJsonRpcClient};
use jsonrpsee::{core::RpcResult, proc_macros::rpc, server::{Server, ServerHandle}, Methods, RpcModule};
use reth::args::RpcServerArgs;
use reth_node_builder::{NodeBuilder, NodeConfig, NodeHandle};
use reth_node_ethereum::EthereumNode;
use reth_provider::{BlockNumReader, BlockReader};
use reth_tasks::TaskManager;
use revm_primitives::U256;
use std::{net::SocketAddr, time::Duration};

#[tokio::test]
async fn bitfinity_test_should_start_local_reth_node() {
    // Arrange
    let _log = init_logs();
    let (reth_address, _reth_node) = start_reth_testing_node(None).await;
    
    let client = ethereum_json_rpc_client::EthJsonRpcClient::new(
        ethereum_json_rpc_client::reqwest::ReqwestClient::new(format!("http://{}", reth_address)),
    );

    // Act & Assert
    assert!(client.get_chain_id().await.is_ok());
}

#[tokio::test]
async fn bitfinity_test_node_forward_get_gas_price_requests() {
    // Arrange
    let _log = init_logs();

    #[rpc(server, namespace = "eth")]
    pub trait Eth {
        #[method(name = "gasPrice")]
        async fn gas_price(&self) -> RpcResult<U256>;
    }

    let gas_price: u128 = rand::random();

    struct EthImpl{
        gas_price: u128
    }

    #[async_trait::async_trait]
    impl EthServer for EthImpl {
        async fn gas_price(&self) -> RpcResult<U256> {
            Ok(U256::from(self.gas_price))
        }
    }

    let (server, eth_server_address) = mock_eth_server_start(EthServer::into_rpc(EthImpl {gas_price})).await;
    println!("Mock Eth server started at: {}", eth_server_address);

    let (reth_address, _reth_node) = start_reth_testing_node(Some(format!("http://{}", eth_server_address))).await;
    
    let client = ethereum_json_rpc_client::EthJsonRpcClient::new(
        ethereum_json_rpc_client::reqwest::ReqwestClient::new(format!("http://{}", reth_address)),
    );

    // Act
    let gas_price_result = client.gas_price().await;

    // Assert
    assert_eq!(gas_price_result.unwrap().as_u128(), gas_price);

}

/// Start a local reth node
async fn start_reth_testing_node(bitfinity_evm_url: Option<String>) -> (SocketAddr, NodeHandle<reth_node_builder::NodeAdapter<reth_node_api::FullNodeTypesAdapter<EthereumNode, std::sync::Arc<reth_db::test_utils::TempDatabase<reth_db::DatabaseEnv>>, reth_provider::providers::BlockchainProvider<std::sync::Arc<reth_db::test_utils::TempDatabase<reth_db::DatabaseEnv>>>>, reth_node_builder::components::Components<reth_node_api::FullNodeTypesAdapter<EthereumNode, std::sync::Arc<reth_db::test_utils::TempDatabase<reth_db::DatabaseEnv>>, reth_provider::providers::BlockchainProvider<std::sync::Arc<reth_db::test_utils::TempDatabase<reth_db::DatabaseEnv>>>>, reth_transaction_pool::Pool<reth_transaction_pool::TransactionValidationTaskExecutor<reth_transaction_pool::EthTransactionValidator<reth_provider::providers::BlockchainProvider<std::sync::Arc<reth_db::test_utils::TempDatabase<reth_db::DatabaseEnv>>>, reth_transaction_pool::EthPooledTransaction>>, reth_transaction_pool::CoinbaseTipOrdering<reth_transaction_pool::EthPooledTransaction>, reth_transaction_pool::blobstore::DiskFileBlobStore>, reth_node_ethereum::EthEvmConfig, reth_node_ethereum::EthExecutorProvider>>>) {
    let tasks = TaskManager::current();



    // create node config
    let node_config = NodeConfig::test()
        .dev()
        .with_rpc(RpcServerArgs::default().with_http().with_http_unused_port());

    let mut chain = node_config.chain.as_ref().clone();
    chain.bitfinity_evm_url = bitfinity_evm_url;

    let node_config = node_config.with_chain(chain);

    let node_handle  = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .node(EthereumNode::default())
        .launch()
        .await.unwrap();

    let address = node_handle.node.rpc_server_handle().http_local_addr().unwrap();
    (address, node_handle)
}

/// Start a local Eth server
async fn mock_eth_server_start(
    methods: impl Into<Methods>,
) -> (ServerHandle, SocketAddr) {

    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let server = Server::builder().build(addr).await.unwrap();

    let mut module = RpcModule::new(());
    module.merge(methods).unwrap();

    let server_address = server.local_addr().unwrap();
    let handle = server.start(module);

    (handle, server_address)
}

/// Stop a local Eth server
async fn mock_eth_server_stop(server: ServerHandle) {
    server.stop().unwrap();
    server.stopped().await;
}