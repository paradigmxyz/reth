//! Run with
//!
//! ```not_rust
//! cargo run -p kona-exex node \
//!     --datadir ./db/ \
//!     --chain sepolia \
//!     --metrics 0.0.0.0:9425 \
//!     --http \
//!     --http.port 8333 \
//!     --http.addr 0.0.0.0 \
//!     --http.api debug,eth,net,trace,txpool,rpc,web3,admin \
//!     --authrpc.jwtsecret <YOUR_JWT_SECRET_FILE> \
//!     --authrpc.addr 0.0.0.0 \
//!     --authrpc.port 8554 \
//!     -vvv
//! ```
//!
//! This launches a regular reth node instance for the specified `--chain`.
//!
//! The Execution Extension runs on top of the node, receiving chain state updates from
//! the reth node. It derives L2 Payload Attributes and validates them against a trusted
//! L2 RPC endpoint.
//!
//! See the [kona-derive][kd] package for details into payload derivation.
//!
//! [kd]: https://github.com/ethereum-optimism/kona/tree/main/crates/derive

use kona_derive::{
    online::*,
    stages::{
        AttributesQueue, BatchQueue, ChannelBank, ChannelReader, FrameQueue, L1Retrieval,
        L1Traversal,
    },
    types::{BlockInfo, L2BlockInfo, OP_MAINNET_CONFIG},
};
use reth::transaction_pool::TransactionPool;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::FullNodeComponents;
use reth_tracing::tracing::{debug, error, info, warn};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

mod blobs;
mod providers;
mod validation;

// L2 RPC is used to validate derived payloads.
const L2_RPC_URL: &str = "L2_RPC_URL";
// Beacon URL is used as a fallback for fetching archived
// blobs that reth won't provide during sync.
const BEACON_URL: &str = "BEACON_URL";

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(reth_node_ethereum::EthereumNode::default())
            .install_exex("kona", move |ctx| async { Ok(follow_safe_head(ctx)) })
            .launch()
            .await?;
        handle.wait_for_node_exit().await
    })
}

fn new_req_url(var: &str) -> reqwest::Url {
    std::env::var(var).unwrap_or_else(|_| panic!("{var} must be set")).parse().unwrap()
}

fn info_from_header(block: &reth::primitives::SealedBlock) -> BlockInfo {
    BlockInfo {
        hash: block.hash(),
        number: block.number,
        timestamp: block.timestamp,
        parent_hash: block.parent_hash,
    }
}

type LocalPipeline =
    DerivationPipeline<LocalAttributesQueue<LocalDataProvider>, AlloyL2ChainProvider>;

type LocalDataProvider = EthereumDataSource<providers::LocalChainProvider, blobs::ExExBlobProvider>;

type LocalAttributesBuilder =
    StatefulAttributesBuilder<providers::LocalChainProvider, AlloyL2ChainProvider>;

type LocalAttributesQueue<DAP> = AttributesQueue<
    BatchQueue<
        ChannelReader<
            ChannelBank<FrameQueue<L1Retrieval<DAP, L1Traversal<providers::LocalChainProvider>>>>,
        >,
        AlloyL2ChainProvider,
    >,
    LocalAttributesBuilder,
>;

pub async fn follow_safe_head(mut ctx: ExExContext<impl FullNodeComponents>) -> eyre::Result<()> {
    let cfg = Arc::new(OP_MAINNET_CONFIG);
    let mut chain_provider = providers::LocalChainProvider::new();
    chain_provider.insert_block_info(BlockInfo {
        hash: cfg.genesis.l1.hash,
        number: cfg.genesis.l1.number,
        timestamp: Default::default(),
        parent_hash: Default::default(),
    });
    let mut l2_provider = AlloyL2ChainProvider::new_http(new_req_url(L2_RPC_URL), cfg.clone());
    let attributes =
        StatefulAttributesBuilder::new(cfg.clone(), l2_provider.clone(), chain_provider.clone());
    let beacon_client = OnlineBeaconClient::new_http(BEACON_URL.to_string());
    let blob_provider =
        OnlineBlobProvider::<_, SimpleSlotDerivation>::new(beacon_client, None, None);
    let blob_store = Arc::new(Mutex::new(blobs::InMemoryBlobProvider::new()));
    let blob_provider = blobs::ExExBlobProvider::new(Arc::clone(&blob_store), blob_provider);
    let dap = EthereumDataSource::new(chain_provider.clone(), blob_provider, &cfg);
    let mut cursor = l2_provider
        .l2_block_info_by_number(cfg.genesis.l2.number)
        .await
        .expect("Failed to fetch genesis L2 block info for pipeline cursor");
    let tip = chain_provider
        .block_info_by_number(cursor.l1_origin.number)
        .await
        .expect("Failed to fetch genesis L1 block info for pipeline tip");
    let validator = validation::OnlineValidator::new_http(
        new_req_url(L2_RPC_URL),
        cfg.canyon_time.unwrap_or_default(),
    );
    let mut pipeline: LocalPipeline = PipelineBuilder::new()
        .rollup_config(cfg.clone())
        .dap_source(dap)
        .l2_chain_provider(l2_provider.clone())
        .chain_provider(chain_provider.clone())
        .builder(attributes)
        .origin(tip)
        .build();
    let mut derived_attributes_count = 0;
    let mut cursor_map: HashMap<BlockInfo, L2BlockInfo> = HashMap::new();

    // Continuously step on the pipeline and validate payloads.
    let mut synced_to_l2_genesis = false;
    let mut curr_tip = 0;
    let mut now = std::time::SystemTime::now();
    loop {
        if synced_to_l2_genesis {
            info!(target: "loop", "Validated payload attributes number {}", derived_attributes_count);
            info!(target: "loop", "Pending l2 safe head num: {}", cursor.block_info.number);
            match pipeline.step(cursor).await {
                Ok(_) => info!(target: "loop", "Stepped derivation pipeline"),
                Err(e) => warn!(target: "loop", "Error stepping derivation pipeline: {:?}", e),
            }

            if let Some(attributes) = pipeline.next_attributes() {
                if !validator.validate(&attributes).await {
                    error!(target: "loop", "Failed payload validation: {}", attributes.parent.block_info.hash);
                    return Ok(());
                }
                derived_attributes_count += 1;
                match l2_provider.l2_block_info_by_number(cursor.block_info.number + 1).await {
                    Ok(bi) => {
                        let tip = chain_provider
                            .block_info_by_number(bi.l1_origin.number)
                            .await
                            .expect("Failed to fetch genesis L1 block info for pipeline tip");
                        cursor_map.insert(tip, bi);
                        cursor = bi;
                    }
                    Err(e) => {
                        error!(target: "loop", "Failed to fetch next pending l2 safe head: {}, err: {:?}", cursor.block_info.number + 1, e);
                    }
                }
                println!(
                    "Validated Payload Attributes {derived_attributes_count} [L2 Block Num: {}] [L2 Timestamp: {}] [L1 Origin Block Num: {}]",
                    attributes.parent.block_info.number + 1,
                    attributes.attributes.timestamp,
                    pipeline.origin().unwrap().number,
                );
                info!(target: "loop", "attributes: {:#?}", attributes);
            } else {
                debug!(target: "loop", "No attributes to validate");
            }
        } else if let Ok(s) = now.elapsed().map(|s| s.as_secs()) {
            if s > 60 {
                now = std::time::SystemTime::now();
                let dist = if curr_tip <= cfg.genesis.l1.number {
                    cfg.genesis.l1.number - curr_tip
                } else {
                    0
                };
                info!(target: "loop", "Not synced to l2 genesis. Distance to genesis: {}", dist);
            }
        }

        if let Ok(notification) = ctx.notifications.try_recv() {
            if let Some(reverted_chain) = notification.reverted_chain() {
                chain_provider.commit(reverted_chain.clone());
                let block_info = info_from_header(&reverted_chain.tip().block);
                let blocks = reverted_chain
                    .blocks_iter()
                    .collect::<Vec<&reth_primitives::SealedBlockWithSenders>>();
                for block in blocks {
                    let tx_hashes = block
                        .transactions()
                        .map(|tx| tx.hash)
                        .collect::<Vec<reth_primitives::TxHash>>();
                    let blobs = ctx.pool().get_all_blobs(tx_hashes)?;
                    let blobs = blobs
                        .into_iter()
                        .map(|b| b.1)
                        .collect::<Vec<reth_primitives::BlobTransactionSidecar>>();
                    let mut locked_blob_provider = blob_store
                        .lock()
                        .map_err(|_| eyre::eyre!("Failed to lock blob provider"))?;
                    let blobs = blobs.into_iter().flat_map(|b| b.blobs).collect();
                    locked_blob_provider.insert_blobs(block.hash(), blobs);
                }
                cursor = if let Some(c) = cursor_map.get(&block_info) {
                    *c
                } else {
                    panic!("Failed to get previous cursor for old chain tip");
                };
                pipeline.reset(block_info).await.map_err(|e| eyre::eyre!(e))?;
            }
            if let Some(committed_chain) = notification.committed_chain() {
                chain_provider.commit(committed_chain.clone());
                let tip_number = committed_chain
                    .blocks_iter()
                    .map(|b| b.block.header.header().number)
                    .max()
                    .unwrap_or_default();
                curr_tip = tip_number;
                if tip_number >= cfg.genesis.l1.number {
                    tracing::debug!(target: "loop", "Chain synced to rollup genesis with L2 block number: {}", tip_number);
                    synced_to_l2_genesis = true;
                }
                ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
            }
        }
        if ctx.notifications.is_closed() {
            warn!(target: "loop", "ExEx notification channel closed, exiting");
            break;
        }
    }

    Err(eyre::eyre!("Main loop exited unexpectedly"))
}
