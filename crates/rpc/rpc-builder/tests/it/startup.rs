//! Startup tests
use crate::utils::{launch_http, launch_ws, test_rpc_builder};
use reth_rpc_builder::{
    error::{RpcError, ServerKind},
    RethRpcModule, RpcServerConfig, TransportRpcModuleConfig,
};
use std::io;

#[tokio::test(flavor = "multi_thread")]
async fn test_http_addr_in_use() {
    let handle = launch_http(vec![RethRpcModule::Admin]).await;
    let addr = handle.http_local_addr().unwrap();
    let builder = test_rpc_builder();
    let server = builder.build(TransportRpcModuleConfig::set_http(vec![RethRpcModule::Admin]));
    let result = server
        .start_server(RpcServerConfig::http(Default::default()).with_http_address(addr))
        .await;
    let expected = RpcError::AddressAlreadyInUse {
        kind: ServerKind::Http(addr),
        error: io::Error::from(io::ErrorKind::AddrInUse),
    };
    assert_eq!(result.unwrap_err(), expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ws_addr_in_use() {
    let handle = launch_ws(vec![RethRpcModule::Admin]).await;
    let addr = handle.ws_local_addr().unwrap();
    let builder = test_rpc_builder();
    let server = builder.build(TransportRpcModuleConfig::set_ws(vec![RethRpcModule::Admin]));
    let result =
        server.start_server(RpcServerConfig::ws(Default::default()).with_ws_address(addr)).await;
    let expected = RpcError::AddressAlreadyInUse {
        kind: ServerKind::WS(addr),
        error: io::Error::from(io::ErrorKind::AddrInUse),
    };
    assert_eq!(result.unwrap_err(), expected);
}
