use alloy_eips::HashOrNumber;
use alloy_primitives::B256;
use example_bsc_sdk::{
    chainspec::{bsc::bsc_mainnet, BscChainSpec},
    node::BscNode,
};
use reth::{
    args::RpcServerArgs,
    builder::{Node, NodeBuilder, NodeConfig, NodeHandle},
    tasks::TaskManager,
};
use reth_eth_wire::HeadersDirection;
use reth_network::{BlockDownloaderProvider, NetworkSyncUpdater, PeersInfo, SyncState};
use reth_network_p2p::{
    headers::client::{HeadersClient, HeadersRequest},
    BodiesClient,
};
use reth_provider::providers::BlockchainProvider;
use std::{sync::Arc, time::Duration};
use tokio::task;

use tracing::info;

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
    let handle = node.network.clone();

    handle.update_sync_state(SyncState::Syncing);
    let h = handle.clone();
    task::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            dbg!(h.num_connected_peers());
        }
    });

    let fetcher = handle.fetch_client().await.unwrap();

    let headers = {
        loop {
            let Ok(headers) = fetcher
                .get_headers(HeadersRequest {
                    start: HashOrNumber::Number(1),
                    limit: 10,
                    direction: HeadersDirection::Rising,
                })
                .await
            else {
                continue
            };

            info!("Received {} headers", headers.1.len());

            if headers.1.len() == 10 {
                break headers.1;
            }
        }
    };

    info!("Successfully retrieved {} headers", headers.len());
    let hashes: Vec<B256> = headers.iter().map(|h| h.hash_slow()).collect::<Vec<_>>();
    let bodies = {
        loop {
            let Ok(bodies) = fetcher.get_block_bodies(hashes.clone()).await else { continue };
            if bodies.1.len() == hashes.len() {
                break bodies;
            }
        }
    };

    info!("Successfully retrieved {} bodies", bodies.1.len());

    Ok(())
}
