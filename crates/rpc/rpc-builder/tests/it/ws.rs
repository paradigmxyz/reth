#![allow(unreachable_pub)]
//! `WebSocket` subscription tests for `eth_subscribe` / `eth_unsubscribe`

use crate::utils::{launch_ws, test_rpc_builder};
use jsonrpsee::core::client::{Subscription, SubscriptionClientT};
use reth_rpc_server_types::RpcModuleSelection;
use reth_tokio_util::EventSender;
use serde_json::Value;
use std::time::Duration;

use reth_rpc_builder::{RpcServerConfig, TransportRpcModuleConfig};

/// Helper to launch a WS server with the Eth module.
async fn launch_ws_eth() -> reth_rpc_builder::RpcServerHandle {
    launch_ws(vec![reth_rpc_server_types::RethRpcModule::Eth]).await
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
    use reth_consensus::noop::NoopConsensus;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::test_utils::NoopProvider;
    use reth_rpc_builder::RpcModuleBuilder;
    use reth_tasks::Runtime;
    use reth_transaction_pool::{
        test_utils::{TestPool, TestPoolBuilder},
        PoolTransaction, TransactionOrigin, TransactionPool,
    };

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
