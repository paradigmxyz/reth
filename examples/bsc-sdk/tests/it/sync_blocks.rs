use alloy_eips::HashOrNumber;
use example_bsc_sdk::{
    chainspec::{
        bsc::{bsc_mainnet, head},
        BscChainSpec,
    },
    node::{
        network::{boot_nodes, handshake::BscHandshake},
        BscNode,
    },
};
use reth::{
    args::RpcServerArgs,
    builder::{Node, NodeBuilder, NodeConfig, NodeHandle},
    tasks::TaskManager,
    transaction_pool::test_utils::testing_pool,
};
use reth_discv4::Discv4Config;
use reth_eth_wire::HeadersDirection;
use reth_network::{
    BlockDownloaderProvider, NetworkEventListenerProvider, NetworkManager, NetworkSyncUpdater,
    PeersInfo, SyncState,
};
use secp256k1::SecretKey;

use reth_network_p2p::headers::client::{HeadersClient, HeadersRequest};
use reth_provider::{noop::NoopProvider, providers::BlockchainProvider};
use secp256k1::rand;
use std::{sync::Arc, time::Duration};
use tokio::task;
use tokio_stream::StreamExt;

#[tokio::test(flavor = "multi_thread")]
async fn can_sync_blocks() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let bsc_chainspec = BscChainSpec { inner: bsc_mainnet() };

    let node_config = NodeConfig::new(Arc::new(bsc_chainspec))
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(exec.clone())
        .with_types_and_provider::<BscNode, BlockchainProvider<_>>()
        .with_components(BscNode::default().components_builder())
        .with_add_ons(BscNode::default().add_ons())
        .launch()
        .await?;
    let h = node.network.clone();
    let mut events = node.network.event_listener();

    h.update_sync_state(SyncState::Syncing);

    task::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            dbg!(h.num_connected_peers());
        }
    });
    while let Some(ev) = events.next().await {
        dbg!(ev);
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_connect_to_trusted_peer() {
    reth_tracing::init_test_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let mut discv4 = Discv4Config::builder();
    discv4.add_boot_nodes(boot_nodes()).lookup_interval(Duration::from_millis(500));

    let client = NoopProvider::eth(Arc::new(bsc_mainnet()));
    let config = reth_network::NetworkConfigBuilder::eth(secret_key)
        .set_head(head())
        .with_pow()
        .eth_rlpx_handshake(Arc::new(BscHandshake::default()))
        .discovery(discv4)
        .build(client.clone());
    let transactions_manager_config = config.transactions_manager_config.clone();
    let (handle, network, transactions, requests) = NetworkManager::new(config)
        .await
        .unwrap()
        .into_builder()
        .request_handler(client)
        .transactions(testing_pool(), transactions_manager_config)
        .split_with_handle();

    let mut events = handle.event_listener();

    tokio::task::spawn(async move {
        tokio::join!(network, requests, transactions);
    });

    let h = handle.clone();
    h.update_sync_state(SyncState::Syncing);

    task::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            dbg!(h.num_connected_peers());
        }
    });

    let fetcher = handle.fetch_client().await.unwrap();

    let headers = fetcher
        .get_headers(HeadersRequest {
            start: HashOrNumber::Number(1),
            limit: 10,
            direction: HeadersDirection::Rising,
        })
        .await;

    dbg!(&headers);

    while let Some(ev) = events.next().await {
        dbg!(ev);
    }
}
