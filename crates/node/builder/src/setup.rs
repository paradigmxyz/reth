//! Helpers for setting up parts of the node.

use reth_config::{config::StageConfig, PruneConfig};
use reth_consensus::Consensus;
use reth_db::database::Database;
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_evm::execute::BlockExecutorProvider;
use reth_exex::ExExManagerHandle;
use reth_interfaces::p2p::{
    bodies::{client::BodiesClient, downloader::BodyDownloader},
    headers::{client::HeadersClient, downloader::HeaderDownloader},
};
use reth_node_core::{
    node_config::NodeConfig,
    primitives::{BlockNumber, B256},
};
use reth_provider::{HeaderSyncMode, ProviderFactory};
use reth_stages::{
    prelude::DefaultStages,
    stages::{
        AccountHashingStage, ExecutionStage, ExecutionStageThresholds, IndexAccountHistoryStage,
        IndexStorageHistoryStage, MerkleStage, SenderRecoveryStage, StorageHashingStage,
        TransactionLookupStage,
    },
    Pipeline, StageSet,
};
use reth_static_file::StaticFileProducer;
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::debug;
use std::sync::Arc;
use tokio::sync::watch;

/// Constructs a [Pipeline] that's wired to the network
#[allow(clippy::too_many_arguments)]
pub async fn build_networked_pipeline<DB, Client, Executor>(
    node_config: &NodeConfig,
    config: &StageConfig,
    client: Client,
    consensus: Arc<dyn Consensus>,
    provider_factory: ProviderFactory<DB>,
    task_executor: &TaskExecutor,
    metrics_tx: reth_stages::MetricEventsSender,
    prune_config: Option<PruneConfig>,
    max_block: Option<BlockNumber>,
    static_file_producer: StaticFileProducer<DB>,
    executor: Executor,
    exex_manager_handle: ExExManagerHandle,
) -> eyre::Result<Pipeline<DB>>
where
    DB: Database + Unpin + Clone + 'static,
    Client: HeadersClient + BodiesClient + Clone + 'static,
    Executor: BlockExecutorProvider,
{
    // building network downloaders using the fetch client
    let header_downloader = ReverseHeadersDownloaderBuilder::new(config.headers)
        .build(client.clone(), Arc::clone(&consensus))
        .into_task_with(task_executor);

    let body_downloader = BodiesDownloaderBuilder::new(config.bodies)
        .build(client, Arc::clone(&consensus), provider_factory.clone())
        .into_task_with(task_executor);

    let pipeline = build_pipeline(
        node_config,
        provider_factory,
        config,
        header_downloader,
        body_downloader,
        consensus,
        max_block,
        metrics_tx,
        prune_config,
        static_file_producer,
        executor,
        exex_manager_handle,
    )
    .await?;

    Ok(pipeline)
}

/// Builds the [Pipeline] with the given [ProviderFactory] and downloaders.
#[allow(clippy::too_many_arguments)]
pub async fn build_pipeline<DB, H, B, Executor>(
    node_config: &NodeConfig,
    provider_factory: ProviderFactory<DB>,
    stage_config: &StageConfig,
    header_downloader: H,
    body_downloader: B,
    consensus: Arc<dyn Consensus>,
    max_block: Option<u64>,
    metrics_tx: reth_stages::MetricEventsSender,
    prune_config: Option<PruneConfig>,
    static_file_producer: StaticFileProducer<DB>,
    executor: Executor,
    exex_manager_handle: ExExManagerHandle,
) -> eyre::Result<Pipeline<DB>>
where
    DB: Database + Clone + 'static,
    H: HeaderDownloader + 'static,
    B: BodyDownloader + 'static,
    Executor: BlockExecutorProvider,
{
    let mut builder = Pipeline::builder();

    if let Some(max_block) = max_block {
        debug!(target: "reth::cli", max_block, "Configuring builder to use max block");
        builder = builder.with_max_block(max_block)
    }

    let (tip_tx, tip_rx) = watch::channel(B256::ZERO);

    let prune_modes = prune_config.map(|prune| prune.segments).unwrap_or_default();

    let header_mode = if node_config.debug.continuous {
        HeaderSyncMode::Continuous
    } else {
        HeaderSyncMode::Tip(tip_rx)
    };
    let pipeline = builder
        .with_tip_sender(tip_tx)
        .with_metrics_tx(metrics_tx.clone())
        .add_stages(
            DefaultStages::new(
                provider_factory.clone(),
                header_mode,
                Arc::clone(&consensus),
                header_downloader,
                body_downloader,
                executor.clone(),
                stage_config.etl.clone(),
            )
            .set(SenderRecoveryStage {
                commit_threshold: stage_config.sender_recovery.commit_threshold,
            })
            .set(
                ExecutionStage::new(
                    executor,
                    ExecutionStageThresholds {
                        max_blocks: stage_config.execution.max_blocks,
                        max_changes: stage_config.execution.max_changes,
                        max_cumulative_gas: stage_config.execution.max_cumulative_gas,
                        max_duration: stage_config.execution.max_duration,
                    },
                    stage_config
                        .merkle
                        .clean_threshold
                        .max(stage_config.account_hashing.clean_threshold)
                        .max(stage_config.storage_hashing.clean_threshold),
                    prune_modes.clone(),
                    exex_manager_handle,
                )
                .with_metrics_tx(metrics_tx),
            )
            .set(AccountHashingStage::new(
                stage_config.account_hashing.clean_threshold,
                stage_config.account_hashing.commit_threshold,
                stage_config.etl.clone(),
            ))
            .set(StorageHashingStage::new(
                stage_config.storage_hashing.clean_threshold,
                stage_config.storage_hashing.commit_threshold,
                stage_config.etl.clone(),
            ))
            .set(MerkleStage::new_execution(stage_config.merkle.clean_threshold))
            .set(TransactionLookupStage::new(
                stage_config.transaction_lookup.chunk_size,
                stage_config.etl.clone(),
                prune_modes.transaction_lookup,
            ))
            .set(IndexAccountHistoryStage::new(
                stage_config.index_account_history.commit_threshold,
                prune_modes.account_history,
                stage_config.etl.clone(),
            ))
            .set(IndexStorageHistoryStage::new(
                stage_config.index_storage_history.commit_threshold,
                prune_modes.storage_history,
                stage_config.etl.clone(),
            )),
        )
        .build(provider_factory, static_file_producer);

    Ok(pipeline)
}
