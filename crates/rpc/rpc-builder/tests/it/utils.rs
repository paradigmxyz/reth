use reth_network_api::test_utils::NoopNetwork;
use reth_provider::test_utils::NoopProvider;
use reth_rpc_builder::{
    RpcModuleBuilder, RpcModuleSelection, RpcServerConfig, RpcServerHandle,
    TransportRpcModuleConfig,
};
use reth_transaction_pool::test_utils::{testing_pool, TestPool};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

/// Localhost with port 0 so a free port is used.
pub fn test_address() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
}

/// Launches a new server with http only with the given modules
pub async fn launch_http(modules: impl Into<RpcModuleSelection>) -> RpcServerHandle {
    let builder = test_rpc_builder();
    let server = builder.build(TransportRpcModuleConfig::http(modules));
    server
        .start_server(RpcServerConfig::http(Default::default()).with_http_address(test_address()))
        .await
        .unwrap()
}

/// Launches a new server with ws only with the given modules
pub async fn launch_ws(modules: impl Into<RpcModuleSelection>) -> RpcServerHandle {
    let builder = test_rpc_builder();
    let server = builder.build(TransportRpcModuleConfig::ws(modules));
    server
        .start_server(RpcServerConfig::ws(Default::default()).with_ws_address(test_address()))
        .await
        .unwrap()
}

/// Launches a new server with http and ws and with the given modules
pub async fn launch_http_ws(modules: impl Into<RpcModuleSelection>) -> RpcServerHandle {
    let builder = test_rpc_builder();
    let modules = modules.into();
    let server = builder.build(TransportRpcModuleConfig::ws(modules.clone()).with_http(modules));
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

/// Returns an [RpcModuleBuilder] with testing components.
pub fn test_rpc_builder() -> RpcModuleBuilder<NoopProvider, TestPool, NoopNetwork> {
    RpcModuleBuilder::default()
        .with_client(NoopProvider::default())
        .with_pool(testing_pool())
        .with_network(NoopNetwork::default())
}
