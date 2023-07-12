use reth_beacon_consensus::BeaconConsensusEngineHandle;
use reth_network_api::noop::NoopNetwork;
use reth_payload_builder::test_utils::spawn_test_payload_service;
use reth_primitives::MAINNET;
use reth_provider::test_utils::{NoopProvider, TestCanonStateSubscriptions};
use reth_rpc::JwtSecret;
use reth_rpc_builder::{
    auth::{AuthRpcModule, AuthServerConfig, AuthServerHandle},
    RpcModuleBuilder, RpcModuleSelection, RpcServerConfig, RpcServerHandle,
    TransportRpcModuleConfig,
};
use reth_rpc_engine_api::EngineApi;
use reth_tasks::TokioTaskExecutor;
use reth_transaction_pool::test_utils::{testing_pool, TestPool};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use tokio::sync::mpsc::unbounded_channel;

/// Localhost with port 0 so a free port is used.
pub fn test_address() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
}

/// Launches a new server for the auth module
pub async fn launch_auth(secret: JwtSecret) -> AuthServerHandle {
    let config = AuthServerConfig::builder(secret).socket_addr(test_address()).build();
    let (tx, _rx) = unbounded_channel();
    let beacon_engine_handle = BeaconConsensusEngineHandle::new(tx);
    let engine_api = EngineApi::new(
        NoopProvider::default(),
        MAINNET.clone(),
        beacon_engine_handle,
        spawn_test_payload_service().into(),
        Box::new(TokioTaskExecutor::default()),
    );
    let module = AuthRpcModule::new(engine_api);
    module.start_server(config).await.unwrap()
}

/// Launches a new server with http only with the given modules
pub async fn launch_http(modules: impl Into<RpcModuleSelection>) -> RpcServerHandle {
    let builder = test_rpc_builder();
    let server = builder.build(TransportRpcModuleConfig::set_http(modules));
    server
        .start_server(RpcServerConfig::http(Default::default()).with_http_address(test_address()))
        .await
        .unwrap()
}

/// Launches a new server with ws only with the given modules
pub async fn launch_ws(modules: impl Into<RpcModuleSelection>) -> RpcServerHandle {
    let builder = test_rpc_builder();
    let server = builder.build(TransportRpcModuleConfig::set_ws(modules));
    server
        .start_server(RpcServerConfig::ws(Default::default()).with_ws_address(test_address()))
        .await
        .unwrap()
}

/// Launches a new server with http and ws and with the given modules
pub async fn launch_http_ws(modules: impl Into<RpcModuleSelection>) -> RpcServerHandle {
    let builder = test_rpc_builder();
    let modules = modules.into();
    let server =
        builder.build(TransportRpcModuleConfig::set_ws(modules.clone()).with_http(modules));
    server
        .start_server(
            RpcServerConfig::ws(Default::default())
                .with_ws_address(test_address())
                .with_http(Default::default())
                .with_http_address(test_address()),
        )
        .await
        .unwrap()
}

/// Launches a new server with http and ws and with the given modules on the same port.
pub async fn launch_http_ws_same_port(modules: impl Into<RpcModuleSelection>) -> RpcServerHandle {
    let builder = test_rpc_builder();
    let modules = modules.into();
    let server =
        builder.build(TransportRpcModuleConfig::set_ws(modules.clone()).with_http(modules));
    let addr = test_address();
    server
        .start_server(
            RpcServerConfig::ws(Default::default())
                .with_ws_address(addr)
                .with_http(Default::default())
                .with_http_address(addr),
        )
        .await
        .unwrap()
}

/// Returns an [RpcModuleBuilder] with testing components.
pub fn test_rpc_builder() -> RpcModuleBuilder<
    NoopProvider,
    TestPool,
    NoopNetwork,
    TokioTaskExecutor,
    TestCanonStateSubscriptions,
> {
    RpcModuleBuilder::default()
        .with_provider(NoopProvider::default())
        .with_pool(testing_pool())
        .with_network(NoopNetwork::default())
        .with_executor(TokioTaskExecutor::default())
        .with_events(TestCanonStateSubscriptions::default())
}
