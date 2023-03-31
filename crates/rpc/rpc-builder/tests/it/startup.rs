//! Startup tests
use crate::utils::{launch_http, launch_ws, test_rpc_builder};
use reth_rpc_builder::{
    error::{RpcError, ServerKind},
    RethRpcModule, RpcServerConfig, TransportRpcModuleConfig,
};
use std::io;

fn is_addr_in_use_kind(err: RpcError, kind: ServerKind) -> bool {
    match err {
        RpcError::AddressAlreadyInUse { kind: k, error } => {
            k == kind && error.kind() == io::ErrorKind::AddrInUse
        }
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_http_addr_in_use() {
    let handle = launch_http(vec![RethRpcModule::Admin]).await;
    let addr = handle.http_local_addr().unwrap();
    let builder = test_rpc_builder();
    let server = builder.build(TransportRpcModuleConfig::set_http(vec![RethRpcModule::Admin]));
    let result = server
        .start_server(RpcServerConfig::http(Default::default()).with_http_address(addr))
        .await;
    assert!(is_addr_in_use_kind(result.unwrap_err(), ServerKind::Http(addr)));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ws_addr_in_use() {
    let handle = launch_ws(vec![RethRpcModule::Admin]).await;
    let addr = handle.ws_local_addr().unwrap();
    let builder = test_rpc_builder();
    let server = builder.build(TransportRpcModuleConfig::set_ws(vec![RethRpcModule::Admin]));
    let result =
        server.start_server(RpcServerConfig::ws(Default::default()).with_ws_address(addr)).await;
    assert!(is_addr_in_use_kind(result.unwrap_err(), ServerKind::WS(addr)));
}
