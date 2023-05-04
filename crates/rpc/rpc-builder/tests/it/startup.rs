//! Startup tests
use crate::utils::{
    launch_http, launch_http_ws_same_port, launch_ws, test_address, test_rpc_builder,
};
use reth_rpc_builder::{
    error::{RpcError, ServerKind, WsHttpSamePortError},
    RethRpcModule, RpcServerConfig, TransportRpcModuleConfig,
};
use std::io;

fn is_addr_in_use_kind(err: &RpcError, kind: ServerKind) -> bool {
    match err {
        RpcError::AddressAlreadyInUse { kind: k, error } => {
            *k == kind && error.kind() == io::ErrorKind::AddrInUse
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
    let err = result.unwrap_err();
    assert!(is_addr_in_use_kind(&err, ServerKind::Http(addr)), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ws_addr_in_use() {
    let handle = launch_ws(vec![RethRpcModule::Admin]).await;
    let addr = handle.ws_local_addr().unwrap();
    let builder = test_rpc_builder();
    let server = builder.build(TransportRpcModuleConfig::set_ws(vec![RethRpcModule::Admin]));
    let result =
        server.start_server(RpcServerConfig::ws(Default::default()).with_ws_address(addr)).await;
    let err = result.unwrap_err();
    assert!(is_addr_in_use_kind(&err, ServerKind::WS(addr)), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_launch_same_port() {
    let handle = launch_http_ws_same_port(vec![RethRpcModule::Admin]).await;
    let ws_addr = handle.ws_local_addr().unwrap();
    let http_addr = handle.http_local_addr().unwrap();
    assert_eq!(ws_addr, http_addr);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_launch_same_port_different_modules() {
    let builder = test_rpc_builder();
    let server = builder.build(
        TransportRpcModuleConfig::set_ws(vec![RethRpcModule::Admin])
            .with_http(vec![RethRpcModule::Eth]),
    );
    let addr = test_address();
    let res = server
        .start_server(
            RpcServerConfig::ws(Default::default())
                .with_ws_address(addr)
                .with_http(Default::default())
                .with_http_address(addr),
        )
        .await;
    let err = res.unwrap_err();
    assert!(matches!(
        err,
        RpcError::WsHttpSamePortError(WsHttpSamePortError::ConflictingModules { .. })
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_launch_same_port_same_cors() {
    let builder = test_rpc_builder();
    let server = builder.build(
        TransportRpcModuleConfig::set_ws(vec![RethRpcModule::Eth])
            .with_http(vec![RethRpcModule::Eth]),
    );
    let addr = test_address();
    let res = server
        .start_server(
            RpcServerConfig::ws(Default::default())
                .with_ws_address(addr)
                .with_http(Default::default())
                .with_cors(Some("*".to_string()))
                .with_http_cors(Some("*".to_string()))
                .with_http_address(addr),
        )
        .await;
    assert!(res.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_launch_same_port_different_cors() {
    let builder = test_rpc_builder();
    let server = builder.build(
        TransportRpcModuleConfig::set_ws(vec![RethRpcModule::Eth])
            .with_http(vec![RethRpcModule::Eth]),
    );
    let addr = test_address();
    let res = server
        .start_server(
            RpcServerConfig::ws(Default::default())
                .with_ws_address(addr)
                .with_http(Default::default())
                .with_cors(Some("*".to_string()))
                .with_http_cors(Some("example".to_string()))
                .with_http_address(addr),
        )
        .await;
    let err = res.unwrap_err();
    assert!(matches!(
        err,
        RpcError::WsHttpSamePortError(WsHttpSamePortError::ConflictingCorsDomains { .. })
    ));
}
