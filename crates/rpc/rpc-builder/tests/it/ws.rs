#![allow(unreachable_pub)]
//! `WebSocket` subscription tests for `eth_subscribe` / `eth_unsubscribe`

use crate::utils::{launch_ws, test_address, test_rpc_builder};
use jsonrpsee::{
    core::client::{Subscription, SubscriptionClientT},
    server::ServerConfigBuilder,
};
use reth_consensus::noop::NoopConsensus;
use reth_evm_ethereum::EthEvmConfig;
use reth_network_api::noop::NoopNetwork;
use reth_primitives_traits::SealedHeader;
use reth_provider::{
    providers::BlockchainProvider,
    test_utils::{create_test_provider_factory, NoopProvider},
};
use reth_rpc_server_types::{RethRpcModule, RpcModuleSelection};
use reth_tasks::Runtime;
use reth_tokio_util::EventSender;
use reth_transaction_pool::{
    test_utils::{TestPool, TestPoolBuilder},
    PoolTransaction, TransactionOrigin, TransactionPool,
};
use serde_json::Value;
use std::time::Duration;
use tokio::time::Instant;

use reth_rpc_builder::{RpcModuleBuilder, RpcServerConfig, TransportRpcModuleConfig};

/// Helper to launch a WS server with the Eth module.
async fn launch_ws_eth() -> reth_rpc_builder::RpcServerHandle {
    launch_ws(vec![reth_rpc_server_types::RethRpcModule::Eth]).await
}

/// Launches a WS server with the Eth module, backed by a provider whose canonical state
/// notification sender stays alive.
///
/// `NoopProvider` drops the sender immediately, which ends canonical state streams right away.
/// With a live sender, subscription tasks block on the stream the same way they do on a running
/// node.
async fn launch_ws_eth_with_canon_state(
    max_subscriptions_per_connection: u32,
) -> reth_rpc_builder::RpcServerHandle {
    let provider =
        BlockchainProvider::with_latest(create_test_provider_factory(), SealedHeader::default())
            .unwrap();
    let pool: TestPool = TestPoolBuilder::default().into();
    let builder = RpcModuleBuilder::default()
        .with_provider(provider)
        .with_pool(pool)
        .with_network(NoopNetwork::default())
        .with_executor(Runtime::test())
        .with_evm_config(EthEvmConfig::mainnet())
        .with_consensus(NoopConsensus::default());
    let eth_api = builder.bootstrap_eth_api();
    let server = builder.build(
        TransportRpcModuleConfig::set_ws(vec![RethRpcModule::Eth]),
        eth_api,
        EventSender::new(1),
    );
    let config = ServerConfigBuilder::default()
        .max_subscriptions_per_connection(max_subscriptions_per_connection);
    RpcServerConfig::ws(config).with_ws_address(test_address()).start(&server).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_subscribe_all_supported_kinds_accept() {
    reth_tracing::init_test_tracing();

    let handle = launch_ws_eth().await;
    let client = handle.ws_client().await.unwrap();

    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("newHeads", vec![]),
        ("newPendingTransactions", vec![]),
        ("newPendingTransactions", vec![serde_json::json!(true)]),
        ("logs", vec![serde_json::json!({})]),
        (
            "logs",
            vec![serde_json::json!({"address": "0x0000000000000000000000000000000000000001"})],
        ),
        (
            "logs",
            vec![
                serde_json::json!({"topics": ["0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"]}),
            ],
        ),
        ("transactionReceipts", vec![]),
        ("transactionReceipts", vec![serde_json::json!({"transactionHashes": []})]),
        (
            "transactionReceipts",
            vec![
                serde_json::json!({"transactionHashes": ["0x5c504ed432cb51138bcf09aa5e8a410dd4a1e204ef84bfed1be16dfba1b22060"]}),
            ],
        ),
    ];

    for (kind, params) in cases {
        let mut rpc_params = jsonrpsee::core::params::ArrayParams::new();
        rpc_params.insert(kind).unwrap();
        for p in params {
            rpc_params.insert(p).unwrap();
        }

        let sub: Subscription<Value> = client
            .subscribe("eth_subscribe", rpc_params, "eth_unsubscribe")
            .await
            .unwrap_or_else(|e| panic!("subscribe({kind}) should succeed: {e}"));

        sub.unsubscribe()
            .await
            .unwrap_or_else(|e| panic!("unsubscribe({kind}) should succeed: {e}"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_subscribe_syncing_delivers_initial_status() {
    reth_tracing::init_test_tracing();

    let handle = launch_ws_eth().await;
    let client = handle.ws_client().await.unwrap();

    let mut sub: Subscription<Value> = client
        .subscribe("eth_subscribe", jsonrpsee::rpc_params!["syncing"], "eth_unsubscribe")
        .await
        .unwrap();

    let initial = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for initial sync status")
        .expect("subscription ended unexpectedly")
        .expect("failed to deserialize sync status");

    // NoopNetwork reports is_syncing = false
    assert_eq!(initial, serde_json::json!(false));

    sub.unsubscribe().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_subscribe_invalid_kind_rejected() {
    reth_tracing::init_test_tracing();

    let handle = launch_ws_eth().await;
    let client = handle.ws_client().await.unwrap();

    let result: Result<Subscription<Value>, _> = client
        .subscribe("eth_subscribe", jsonrpsee::rpc_params!["invalidKind"], "eth_unsubscribe")
        .await;

    assert!(result.is_err(), "invalid subscription kind must be rejected");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_subscribe_server_survives_client_disconnect() {
    reth_tracing::init_test_tracing();

    let handle = launch_ws_eth().await;

    {
        let client = handle.ws_client().await.unwrap();
        let _sub: Subscription<Value> = client
            .subscribe("eth_subscribe", jsonrpsee::rpc_params!["newHeads"], "eth_unsubscribe")
            .await
            .unwrap();
        // client + subscription drop here
    }

    // Server must still accept new connections after a client disconnects
    let client2 = handle.ws_client().await.unwrap();
    let sub: Subscription<Value> = client2
        .subscribe("eth_subscribe", jsonrpsee::rpc_params!["newHeads"], "eth_unsubscribe")
        .await
        .unwrap();

    sub.unsubscribe().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_subscribe_not_available_over_http() {
    reth_tracing::init_test_tracing();

    let builder = test_rpc_builder();
    let eth_api = builder.bootstrap_eth_api();
    let modules = RpcModuleSelection::Standard;
    let server =
        builder.build(TransportRpcModuleConfig::set_http(modules), eth_api, EventSender::new(1));
    let handle = RpcServerConfig::http(Default::default())
        .with_http_address(crate::utils::test_address())
        .start(&server)
        .await
        .unwrap();

    assert!(handle.ws_client().await.is_none(), "WS should not be available on HTTP-only server");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_subscribe_pending_transactions_receives_tx() {
    reth_tracing::init_test_tracing();

    let pool: TestPool = TestPoolBuilder::default().into();
    let pool_clone = pool.clone();

    let builder = RpcModuleBuilder::default()
        .with_provider(NoopProvider::default())
        .with_pool(pool)
        .with_network(NoopNetwork::default())
        .with_executor(Runtime::test())
        .with_evm_config(EthEvmConfig::mainnet())
        .with_consensus(NoopConsensus::default());

    let eth_api = builder.bootstrap_eth_api();
    let server = builder.build(
        TransportRpcModuleConfig::set_ws(RpcModuleSelection::Standard),
        eth_api,
        EventSender::new(1),
    );
    let handle = RpcServerConfig::ws(Default::default())
        .with_ws_address(crate::utils::test_address())
        .start(&server)
        .await
        .unwrap();

    let client = handle.ws_client().await.unwrap();

    // Subscribe to pending transaction hashes
    let mut sub: Subscription<Value> = client
        .subscribe(
            "eth_subscribe",
            jsonrpsee::rpc_params!["newPendingTransactions"],
            "eth_unsubscribe",
        )
        .await
        .unwrap();

    // Insert a transaction into the pool
    let tx = reth_transaction_pool::test_utils::MockTransaction::eip1559();
    let expected_hash = *tx.hash();
    pool_clone.add_transaction(TransactionOrigin::External, tx).await.unwrap();

    // We should receive the tx hash via the subscription
    let received = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for pending tx notification")
        .expect("subscription ended unexpectedly")
        .expect("failed to deserialize tx hash");

    let received_hash: alloy_primitives::TxHash = serde_json::from_value(received).unwrap();
    assert_eq!(received_hash, expected_hash);

    sub.unsubscribe().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_subscribe_syncing_releases_permit_on_unsubscribe() {
    reth_tracing::init_test_tracing();

    // A single permit means each subscription must release it before the next one is accepted.
    let handle = launch_ws_eth_with_canon_state(1).await;
    let client = handle.ws_client().await.unwrap();

    for i in 0..3 {
        // The previous subscription task releases its permit asynchronously after unsubscribe.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut sub: Subscription<Value> = loop {
            match client
                .subscribe("eth_subscribe", jsonrpsee::rpc_params!["syncing"], "eth_unsubscribe")
                .await
            {
                Ok(sub) => break sub,
                Err(err) if Instant::now() >= deadline => {
                    panic!("syncing subscription #{i} rejected after unsubscribe: {err}")
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        };

        let initial = tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .expect("timed out waiting for initial syncing status")
            .expect("subscription closed before initial status")
            .unwrap();
        assert_eq!(initial, Value::Bool(false));

        sub.unsubscribe().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_subscribe_syncing_task_exits_on_disconnect() {
    reth_tracing::init_test_tracing();

    const SUBSCRIPTIONS: usize = 50;

    let metrics = tokio::runtime::Handle::current().metrics();
    let handle = launch_ws_eth_with_canon_state(SUBSCRIPTIONS as u32).await;

    // Open and close one connection first so tasks spawned lazily by the server are not counted.
    drop(handle.ws_client().await.unwrap());
    tokio::time::sleep(Duration::from_millis(500)).await;
    let baseline = metrics.num_alive_tasks();

    let client = handle.ws_client().await.unwrap();
    let mut subs = Vec::with_capacity(SUBSCRIPTIONS);
    for _ in 0..SUBSCRIPTIONS {
        let mut sub: Subscription<Value> = client
            .subscribe("eth_subscribe", jsonrpsee::rpc_params!["syncing"], "eth_unsubscribe")
            .await
            .unwrap();
        assert_eq!(sub.next().await.unwrap().unwrap(), Value::Bool(false));
        subs.push(sub);
    }
    assert!(metrics.num_alive_tasks() >= baseline + SUBSCRIPTIONS);

    // Close the connection before dropping the subscriptions so no `eth_unsubscribe` is sent.
    drop(client);
    drop(subs);

    let deadline = Instant::now() + Duration::from_secs(5);
    while metrics.num_alive_tasks() > baseline && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let alive = metrics.num_alive_tasks();
    assert!(alive <= baseline, "subscription tasks outlived the connection: {baseline} -> {alive}");
}
