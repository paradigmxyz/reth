//! Startup tests

use alloy_rpc_types_engine::Claims;
use http::header::ACCESS_CONTROL_ALLOW_ORIGIN;
use reth_rpc_layer::JwtSecret;
use std::{
    collections::{HashMap, HashSet},
    io,
    time::{SystemTime, UNIX_EPOCH},
};

use jsonrpsee::Methods;
use reth_rpc_builder::{
    error::{RpcError, ServerKind, WsHttpSamePortError},
    RpcServerConfig, TransportRpcModuleConfig,
};
use reth_rpc_server_types::RethRpcModule;
use reth_tokio_util::EventSender;

use crate::utils::{
    launch_http, launch_http_ws_same_port, launch_ws, test_address, test_rpc_builder,
};

fn is_addr_in_use_kind(err: &RpcError, kind: ServerKind) -> bool {
    match err {
        RpcError::AddressAlreadyInUse { kind: k, error } => {
            *k == kind && error.kind() == io::ErrorKind::AddrInUse
        }
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_rpc_modules_per_transport() {
    let builder = test_rpc_builder();
    let eth_api = builder.bootstrap_eth_api();
    let modules = builder.build(
        TransportRpcModuleConfig::set_http(vec![RethRpcModule::Rpc, RethRpcModule::Eth])
            .with_ws(vec![RethRpcModule::Rpc, RethRpcModule::Net]),
        eth_api,
        EventSender::new(1),
    );

    let namespaces = |methods: Methods| async move {
        let (response, _) = methods
            .raw_json_request(r#"{"jsonrpc":"2.0","id":1,"method":"rpc_modules"}"#, 1)
            .await
            .unwrap();
        let response = serde_json::from_str::<serde_json::Value>(&response).unwrap();
        let modules =
            serde_json::from_value::<HashMap<String, String>>(response["result"].clone()).unwrap();
        modules.into_keys().collect::<HashSet<_>>()
    };
    let expected = |names: [&str; 2]| names.into_iter().map(String::from).collect::<HashSet<_>>();

    assert_eq!(namespaces(modules.http_methods(|_| true).unwrap()).await, expected(["eth", "rpc"]));
    assert_eq!(namespaces(modules.ws_methods(|_| true).unwrap()).await, expected(["net", "rpc"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_http_addr_in_use() {
    let handle = launch_http(vec![RethRpcModule::Admin]).await;
    let addr = handle.http_local_addr().unwrap();
    let builder = test_rpc_builder();
    let eth_api = builder.bootstrap_eth_api();
    let server = builder.build(
        TransportRpcModuleConfig::set_http(vec![RethRpcModule::Admin]),
        eth_api,
        EventSender::new(1),
    );
    let result =
        RpcServerConfig::http(Default::default()).with_http_address(addr).start(&server).await;
    let err = result.unwrap_err();
    assert!(is_addr_in_use_kind(&err, ServerKind::Http(addr)), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ws_addr_in_use() {
    let handle = launch_ws(vec![RethRpcModule::Admin]).await;
    let addr = handle.ws_local_addr().unwrap();
    let builder = test_rpc_builder();
    let eth_api = builder.bootstrap_eth_api();
    let server = builder.build(
        TransportRpcModuleConfig::set_ws(vec![RethRpcModule::Admin]),
        eth_api,
        EventSender::new(1),
    );
    let result = RpcServerConfig::ws(Default::default()).with_ws_address(addr).start(&server).await;
    let err = result.unwrap_err();
    assert!(is_addr_in_use_kind(&err, ServerKind::WS(addr)), "{err}");
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
    let eth_api = builder.bootstrap_eth_api();
    let server = builder.build(
        TransportRpcModuleConfig::set_ws(vec![RethRpcModule::Admin])
            .with_http(vec![RethRpcModule::Eth]),
        eth_api,
        EventSender::new(1),
    );
    let addr = test_address();
    let res = RpcServerConfig::ws(Default::default())
        .with_ws_address(addr)
        .with_http(Default::default())
        .with_http_address(addr)
        .start(&server)
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
    let eth_api = builder.bootstrap_eth_api();
    let server = builder.build(
        TransportRpcModuleConfig::set_ws(vec![RethRpcModule::Eth])
            .with_http(vec![RethRpcModule::Eth]),
        eth_api,
        EventSender::new(1),
    );
    let addr = test_address();
    let res = RpcServerConfig::ws(Default::default())
        .with_ws_address(addr)
        .with_http(Default::default())
        .with_cors(Some("*".to_string()))
        .with_http_cors(Some("*".to_string()))
        .with_http_address(addr)
        .start(&server)
        .await;
    assert!(res.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_launch_same_port_different_cors() {
    let builder = test_rpc_builder();
    let eth_api = builder.bootstrap_eth_api();
    let server = builder.build(
        TransportRpcModuleConfig::set_ws(vec![RethRpcModule::Eth])
            .with_http(vec![RethRpcModule::Eth]),
        eth_api,
        EventSender::new(1),
    );
    let addr = test_address();
    let res = RpcServerConfig::ws(Default::default())
        .with_ws_address(addr)
        .with_http(Default::default())
        .with_cors(Some("*".to_string()))
        .with_http_cors(Some("example".to_string()))
        .with_http_address(addr)
        .start(&server)
        .await;
    let err = res.unwrap_err();
    assert!(matches!(
        err,
        RpcError::WsHttpSamePortError(WsHttpSamePortError::ConflictingCorsDomains { .. })
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ws_upgrade_origin_allowlist() {
    for with_jwt in [false, true] {
        for domains in [None, Some("*"), Some("https://allowed.example")] {
            for (same_port, http_domains, ws_domains) in
                [(false, None, domains), (true, domains, None), (true, None, domains)]
            {
                let secret = with_jwt.then(JwtSecret::random);
                let token = secret.map(|secret| {
                    secret
                        .encode(&Claims {
                            iat: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                            exp: None,
                        })
                        .unwrap()
                });
                let builder = test_rpc_builder();
                let eth_api = builder.bootstrap_eth_api();
                let modules = builder.build(
                    TransportRpcModuleConfig::set_ws(vec![RethRpcModule::Web3])
                        .with_http(vec![RethRpcModule::Web3]),
                    eth_api,
                    EventSender::new(1),
                );
                let addr = test_address();
                let mut config = RpcServerConfig::ws(Default::default())
                    .with_ws_address(addr)
                    .with_ws_cors(ws_domains.map(str::to_string))
                    .with_jwt_secret(secret);
                if same_port {
                    config = config
                        .with_http(Default::default())
                        .with_http_address(addr)
                        .with_http_cors(http_domains.map(str::to_string));
                }
                let handle = config.start(&modules).await.unwrap();
                let url = format!("http://{}", handle.ws_local_addr().unwrap());
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                    .unwrap();
                if with_jwt {
                    assert_eq!(
                        client.get(&url).send().await.unwrap().status(),
                        http::StatusCode::UNAUTHORIZED
                    );
                }
                for (origins, allowed_by_default) in [
                    (vec![], true),
                    (vec!["http://localhost:3000"], true),
                    (vec!["https://127.0.0.1"], true),
                    (vec!["http://[::1]:8080"], true),
                    (vec!["https://[::1]"], true),
                    (vec!["https://allowed.example"], false),
                    (vec!["https://denied.example"], false),
                    (vec!["null"], false),
                    (vec!["https://localhost.example"], false),
                    (vec!["file://localhost"], false),
                    (vec!["http://localhost:65536"], false),
                    (vec!["http://localhost:+80"], false),
                    (vec!["http://localhost/path"], false),
                    (vec!["http://localhost", "http://localhost"], false),
                    (vec!["https://allowed.example", "https://denied.example"], false),
                ] {
                    let mut request = client
                        .get(&url)
                        .header("connection", "Upgrade")
                        .header("upgrade", "websocket")
                        .header("sec-websocket-version", "13")
                        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
                    if let Some(token) = &token {
                        request = request.bearer_auth(token);
                    }
                    for origin in &origins {
                        request = request.header("origin", *origin);
                    }
                    let response = request.send().await.unwrap();
                    let allowed = match domains {
                        None => allowed_by_default,
                        Some("*") => true,
                        Some(domain) => origins.is_empty() || origins == [domain],
                    };
                    assert_eq!(
                        response.status(),
                        if allowed {
                            http::StatusCode::SWITCHING_PROTOCOLS
                        } else {
                            http::StatusCode::FORBIDDEN
                        },
                        "same_port={same_port}, http={http_domains:?}, ws={ws_domains:?}, origins={origins:?}"
                    );
                }
                if same_port {
                    // Ordinary HTTP still relies on response CORS headers, not request rejection.
                    for origin in [
                        "http://localhost:3000",
                        "https://allowed.example",
                        "https://denied.example",
                    ] {
                        let mut request = client
                            .post(&url)
                            .header("origin", origin)
                            .header("content-type", "application/json")
                            .body(
                                r#"{"jsonrpc":"2.0","id":1,"method":"web3_clientVersion","params":[]}"#,
                            );
                        if let Some(token) = &token {
                            request = request.bearer_auth(token);
                        }
                        let response = request.send().await.unwrap();
                        assert_eq!(response.status(), http::StatusCode::OK);
                        assert_eq!(
                            response
                                .headers()
                                .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                                .map(|v| v.to_str().unwrap()),
                            domains.filter(|&domain| domain == "*" || domain == origin),
                            "http={http_domains:?}, ws={ws_domains:?}, origin={origin}"
                        );
                    }
                }
                handle.stop().unwrap();
            }
        }
    }
}
