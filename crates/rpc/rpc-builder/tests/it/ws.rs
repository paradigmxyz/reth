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
    test_utils::{
        blocks::BlockchainTestData, create_test_provider_factory, MockNodeTypesWithDB, NoopProvider,
    },
    CanonStateNotification, Chain,
};
use reth_rpc_server_types::{RethRpcModule, RpcModuleSelection};
use reth_tasks::Runtime;
use reth_tokio_util::EventSender;
use reth_transaction_pool::{
    test_utils::{TestPool, TestPoolBuilder},
    PoolTransaction, TransactionOrigin, TransactionPool,
};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
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
    launch_ws_eth_with_provider(canon_state_provider(), max_subscriptions_per_connection).await
}

/// Returns a provider whose canonical state notification sender stays alive.
fn canon_state_provider() -> BlockchainProvider<MockNodeTypesWithDB> {
    BlockchainProvider::with_latest(create_test_provider_factory(), SealedHeader::default())
        .unwrap()
}

/// Launches a WS server with the Eth module, backed by the given provider.
async fn launch_ws_eth_with_provider(
    provider: BlockchainProvider<MockNodeTypesWithDB>,
    max_subscriptions_per_connection: u32,
) -> reth_rpc_builder::RpcServerHandle {
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

/// Launches a WS server with the standard modules, backed by the given pool.
async fn launch_ws_with_pool(pool: TestPool) -> reth_rpc_builder::RpcServerHandle {
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
    RpcServerConfig::ws(Default::default())
        .with_ws_address(crate::utils::test_address())
        .start(&server)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_subscribe_pending_transactions_receives_tx() {
    reth_tracing::init_test_tracing();

    let pool: TestPool = TestPoolBuilder::default().into();
    let pool_clone = pool.clone();
    let handle = launch_ws_with_pool(pool).await;
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

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_subscribe_unfiltered_logs_match_filtered_logs() {
    reth_tracing::init_test_tracing();

    let provider = canon_state_provider();
    let handle = launch_ws_eth_with_provider(provider.clone(), 8).await;
    let client = handle.ws_client().await.unwrap();

    // Unfiltered subscriptions share one feed, while a block range makes the filter non-default
    // without excluding any log, so that subscription encodes its own logs.
    let mut subs = Vec::new();
    for params in [
        jsonrpsee::rpc_params!["logs"],
        jsonrpsee::rpc_params!["logs", serde_json::json!({})],
        jsonrpsee::rpc_params!["logs", serde_json::json!({"fromBlock": "0x1"})],
    ] {
        let sub: Subscription<Value> =
            client.subscribe("eth_subscribe", params, "eth_unsubscribe").await.unwrap();
        subs.push(sub);
    }

    let mut blocks = BlockchainTestData::default().blocks.into_iter();
    let (first, mut outcome) = blocks.next().unwrap();
    let (second, second_outcome) = blocks.next().unwrap();
    outcome.extend(second_outcome);
    let num_logs = outcome.receipts.iter().flatten().map(|receipt| receipt.logs.len()).sum();
    let commit = CanonStateNotification::Commit {
        new: Arc::new(Chain::new([first, second], outcome, BTreeMap::new())),
    };

    // Subscription tasks subscribe to canonical state asynchronously, so keep committing the same
    // chain until every subscription received its logs.
    let in_memory_state = provider.canonical_in_memory_state();
    let notifier = tokio::spawn(async move {
        loop {
            in_memory_state.notify_canon_state(commit.clone());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    let mut received = Vec::new();
    for sub in &mut subs {
        let mut logs = Vec::with_capacity(num_logs);
        while logs.len() < num_logs {
            let log = tokio::time::timeout(Duration::from_secs(5), sub.next())
                .await
                .expect("timed out waiting for logs")
                .expect("subscription ended unexpectedly")
                .unwrap();
            logs.push(log);
        }
        received.push(logs);
    }
    notifier.abort();

    assert!(num_logs > 1);
    assert_eq!(received[0], received[2]);
    assert_eq!(received[1], received[2]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_subscribe_full_pending_transactions_reach_every_subscriber() {
    reth_tracing::init_test_tracing();

    let pool: TestPool = TestPoolBuilder::default().into();
    let handle = launch_ws_with_pool(pool.clone()).await;
    let client = handle.ws_client().await.unwrap();

    let mut subs = Vec::new();
    for _ in 0..2 {
        let sub: Subscription<Value> = client
            .subscribe(
                "eth_subscribe",
                jsonrpsee::rpc_params!["newPendingTransactions", true],
                "eth_unsubscribe",
            )
            .await
            .unwrap();
        subs.push(sub);
    }

    // Subscription tasks join the shared feed asynchronously, so keep adding transactions until
    // one reaches both subscriptions.
    let mut senders = Vec::new();
    let mut received = [Vec::new(), Vec::new()];
    for _ in 0..50 {
        let tx = reth_transaction_pool::test_utils::MockTransaction::eip1559();
        senders.push(tx.sender());
        pool.add_transaction(TransactionOrigin::External, tx).await.unwrap();

        for (sub, received) in subs.iter_mut().zip(&mut received) {
            if let Ok(Some(tx)) = tokio::time::timeout(Duration::from_millis(100), sub.next()).await
            {
                received.push(tx.unwrap());
            }
        }

        if let Some(tx) = received[0].iter().find(|tx| received[1].contains(tx)) {
            let from: alloy_primitives::Address =
                serde_json::from_value(tx["from"].clone()).unwrap();
            assert!(senders.contains(&from));
            return
        }
    }
    panic!("no pending transaction reached both subscriptions");
}
