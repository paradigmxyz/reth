use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use alloy_rpc_types_engine::{ClientCode, ClientVersionV1};
use reth_chainspec::MAINNET;
use reth_consensus::noop::NoopConsensus;
use reth_engine_primitives::BeaconConsensusEngineHandle;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::execute::BasicBlockExecutorProvider;
use reth_evm_ethereum::EthEvmConfig;
use reth_network_api::noop::NoopNetwork;
use reth_node_ethereum::EthereumEngineValidator;
use reth_payload_builder::test_utils::spawn_test_payload_service;
use reth_provider::test_utils::NoopProvider;
use reth_rpc_builder::{
    auth::{AuthRpcModule, AuthServerConfig, AuthServerHandle},
    RpcModuleBuilder, RpcServerConfig, RpcServerHandle, TransportRpcModuleConfig,
};
use reth_rpc_engine_api::{capabilities::EngineCapabilities, EngineApi};
use reth_rpc_layer::JwtSecret;
use reth_rpc_server_types::RpcModuleSelection;
use reth_tasks::TokioTaskExecutor;
use reth_transaction_pool::{
    noop::NoopTransactionPool,
    test_utils::{TestPool, TestPoolBuilder},
};
use tokio::sync::mpsc::unbounded_channel;

/// Localhost with port 0 so a free port is used.
pub const fn test_address() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
}

/// Launches a new server for the auth module
pub async fn launch_auth(secret: JwtSecret) -> AuthServerHandle {
    let config = AuthServerConfig::builder(secret).socket_addr(test_address()).build();
    let (tx, _rx) = unbounded_channel();
    let beacon_engine_handle = BeaconConsensusEngineHandle::<EthEngineTypes>::new(tx);
    let client = ClientVersionV1 {
        code: ClientCode::RH,
        name: "Reth".to_string(),
        version: "v0.2.0-beta.5".to_string(),
        commit: "defa64b2".to_string(),
    };

    let engine_api = EngineApi::new(
        NoopProvider::default(),
        MAINNET.clone(),
        beacon_engine_handle,
        spawn_test_payload_service().into(),
        NoopTransactionPool::default(),
        Box::<TokioTaskExecutor>::default(),
        client,
        EngineCapabilities::default(),
        EthereumEngineValidator::new(MAINNET.clone()),
        false,
    );
    let module = AuthRpcModule::new(engine_api);
    module.start_server(config).await.unwrap()
}

/// Launches a new server with http only with the given modules
pub async fn launch_http(modules: impl Into<RpcModuleSelection>) -> RpcServerHandle {
    let builder = test_rpc_builder();
    let eth_api = builder.bootstrap_eth_api();
    let server = builder.build(TransportRpcModuleConfig::set_http(modules), eth_api);
    RpcServerConfig::http(Default::default())
        .with_http_address(test_address())
        .start(&server)
        .await
        .unwrap()
}

/// Launches a new server with ws only with the given modules
pub async fn launch_ws(modules: impl Into<RpcModuleSelection>) -> RpcServerHandle {
    let builder = test_rpc_builder();
    let eth_api = builder.bootstrap_eth_api();
    let server = builder.build(TransportRpcModuleConfig::set_ws(modules), eth_api);
    RpcServerConfig::ws(Default::default())
        .with_ws_address(test_address())
        .start(&server)
        .await
        .unwrap()
}

/// Launches a new server with http and ws and with the given modules
pub async fn launch_http_ws(modules: impl Into<RpcModuleSelection>) -> RpcServerHandle {
    let builder = test_rpc_builder();
    let eth_api = builder.bootstrap_eth_api();
    let modules = modules.into();
    let server = builder
        .build(TransportRpcModuleConfig::set_ws(modules.clone()).with_http(modules), eth_api);
    RpcServerConfig::ws(Default::default())
        .with_ws_address(test_address())
        .with_ws_address(test_address())
        .with_http(Default::default())
        .with_http_address(test_address())
        .start(&server)
        .await
        .unwrap()
}

/// Launches a new server with http and ws and with the given modules on the same port.
pub async fn launch_http_ws_same_port(modules: impl Into<RpcModuleSelection>) -> RpcServerHandle {
    let builder = test_rpc_builder();
    let modules = modules.into();
    let eth_api = builder.bootstrap_eth_api();
    let server = builder
        .build(TransportRpcModuleConfig::set_ws(modules.clone()).with_http(modules), eth_api);
    let addr = test_address();
    RpcServerConfig::ws(Default::default())
        .with_ws_address(addr)
        .with_http(Default::default())
        .with_http_address(addr)
        .start(&server)
        .await
        .unwrap()
}

/// Returns an [`RpcModuleBuilder`] with testing components.
pub fn test_rpc_builder() -> RpcModuleBuilder<
    EthPrimitives,
    NoopProvider,
    TestPool,
    NoopNetwork,
    TokioTaskExecutor,
    EthEvmConfig,
    BasicBlockExecutorProvider<EthEvmConfig>,
    NoopConsensus,
> {
    RpcModuleBuilder::default()
        .with_provider(NoopProvider::default())
        .with_pool(TestPoolBuilder::default().into())
        .with_network(NoopNetwork::default())
        .with_executor(TokioTaskExecutor::default())
        .with_evm_config(EthEvmConfig::mainnet())
        .with_block_executor(BasicBlockExecutorProvider::new(EthEvmConfig::mainnet()))
        .with_consensus(NoopConsensus::default())
}
