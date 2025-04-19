use alloy_consensus::Block;
use alloy_eips::HashOrNumber;
use alloy_primitives::{B256, U128};
use example_bsc_sdk::{
    chainspec::{bsc::bsc_mainnet, BscChainSpec},
    consensus::ParliaConsensus,
    node::{network::block_import::service::ImportService as BlockImportService, BscNode},
};
use futures::StreamExt;
use reth::{
    args::RpcServerArgs,
    builder::{Node, NodeBuilder, NodeConfig, NodeHandle},
    tasks::TaskManager,
};
use reth_eth_wire::HeadersDirection;
use reth_eth_wire_types::NewBlock;
use reth_network::{
    message::NewBlockMessage, BlockDownloaderProvider, NetworkSyncUpdater, PeersInfo, SyncState,
};
use reth_network_api::PeerId;
use reth_network_p2p::{
    headers::client::{HeadersClient, HeadersRequest},
    BodiesClient,
};
use reth_primitives::TransactionSigned;
use reth_provider::{providers::BlockchainProvider, CanonStateSubscriptions};
use std::{fs::File, sync::Arc, time::Duration};
use tokio::task;
use tracing::{error, info};

#[tokio::test(flavor = "multi_thread")]
async fn can_sync_from_file() -> eyre::Result<()> {
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
    let provider = node.provider.clone();
    let consensus = Arc::new(ParliaConsensus { provider: provider.clone() });

    let (service, block_handle) =
        BlockImportService::new(consensus, node.beacon_engine_handle.clone());

    tokio::spawn(Box::pin(async move {
        if let Err(e) = service.await {
            error!("Import service error: {}", e);
        }
    }));

    let start_block: u64 = 1;
    let end_block: u64 = 10;

    let blocks = read_blocks_from_file("blocks.json", end_block - start_block + 1).unwrap();
    info!("Successfully read {} blocks from {}", blocks.len(), "blocks.json");

    let mut notifications = provider.canonical_state_stream();
    let mut current_block = start_block - 1;

    for b in &blocks {
        let td = U128::from(b.header.difficulty);
        let block = NewBlockMessage {
            hash: b.header.hash_slow(),
            block: Arc::new(NewBlock { block: b.clone(), td }),
        };

        if let Err(err) = block_handle.send_block(block, PeerId::random()) {
            error!("Error sending block: {}", err);
        }

        // Wait for the block to be imported
        let head = notifications.next().await.unwrap();
        let block_number = head.tip().header().number;
        let block_hash = head.tip().header().hash_slow();

        assert_eq!(block_number, current_block + 1);
        assert_eq!(block_hash, b.header.hash_slow());
        current_block += 1;
    }

    assert_eq!(current_block, end_block);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn can_sync_from_p2p() -> eyre::Result<()> {
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
    let provider = node.provider.clone();
    let consensus = Arc::new(ParliaConsensus { provider: provider.clone() });

    let (service, block_handle) =
        BlockImportService::new(consensus, node.beacon_engine_handle.clone());

    tokio::spawn(Box::pin(async move {
        if let Err(e) = service.await {
            error!("Import service error: {}", e);
        }
    }));

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

    let start_block = 1;
    let end_block = 2000;
    let limit = end_block - start_block + 1;

    let headers = {
        loop {
            let Ok(headers) = fetcher
                .get_headers(HeadersRequest {
                    start: HashOrNumber::Number(start_block),
                    limit,
                    direction: HeadersDirection::Rising,
                })
                .await
            else {
                continue
            };

            info!("Received {} headers", headers.1.len());

            if headers.1.len() == limit as usize {
                break headers.1;
            }
        }
    };

    info!("Successfully retrieved {} headers", headers.len());
    let hashes: Vec<B256> = headers.iter().map(|h| h.hash_slow()).collect::<Vec<_>>();
    let bodies: Vec<Block<TransactionSigned>> = {
        loop {
            let Ok(bodies) = fetcher.get_block_bodies(hashes.clone()).await else { continue };
            if bodies.1.len() == hashes.len() {
                let mut blocks = Vec::new();
                for (i, b) in bodies.1.iter().enumerate() {
                    let header = headers[i].clone();
                    let block = b.clone().into_block(header);
                    blocks.push(block);
                }
                break blocks;
            }
        }
    };

    info!("Successfully retrieved {} bodies", bodies.len());
    let filename = format!("{}_{}_blocks.json", start_block, end_block);
    let file = File::create(filename).unwrap();
    serde_json::to_writer(file, &bodies).unwrap();

    let mut notifications = provider.canonical_state_stream();
    let mut current_block = start_block - 1;

    for b in bodies {
        let td = U128::from(b.header.difficulty);
        let block = NewBlockMessage {
            hash: b.header.hash_slow(),
            block: Arc::new(NewBlock { block: b.clone(), td }),
        };

        if let Err(err) = block_handle.send_block(block, PeerId::random()) {
            error!("Error sending block: {}", err);
        }

        // Wait for the block to be imported
        let head = notifications.next().await.unwrap();
        let block_number = head.tip().header().number;
        let block_hash = head.tip().header().hash_slow();

        assert_eq!(block_number, current_block + 1);
        assert_eq!(block_hash, b.header.hash_slow());
        current_block += 1;
    }

    assert_eq!(current_block, end_block);
    Ok(())
}

fn read_blocks_from_file<P: AsRef<std::path::Path>>(
    path: P,
    num_blocks: u64,
) -> Result<Vec<Block<TransactionSigned>>, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let blocks: Vec<Block<TransactionSigned>> = serde_json::from_reader(file)?;
    Ok(blocks.into_iter().take(num_blocks as usize).collect())
}
