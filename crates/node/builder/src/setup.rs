//! Helpers for setting up parts of the node.

use std::sync::Arc;

use alloy_primitives::{BlockNumber, B256};
use reth_config::{config::StageConfig, PruneConfig};
use reth_consensus::Consensus;
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_evm::execute::BlockExecutorProvider;
use reth_exex::ExExManagerHandle;
use reth_network_p2p::{
    bodies::downloader::BodyDownloader, headers::downloader::HeaderDownloader, BlockClient,
};
use reth_provider::{providers::ProviderNodeTypes, ProviderFactory};
use reth_stages::{prelude::DefaultStages, stages::ExecutionStage, Pipeline, StageSet};
use reth_static_file::StaticFileProducer;
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::debug;
use tokio::sync::watch;

/// Constructs a [Pipeline] that's wired to the network
#[allow(clippy::too_many_arguments)]
pub fn build_networked_pipeline<N, Client, Executor>(
    config: &StageConfig,
    client: Client,
    consensus: Arc<dyn Consensus>,
    provider_factory: ProviderFactory<N>,
    task_executor: &TaskExecutor,
    metrics_tx: reth_stages::MetricEventsSender,
    prune_config: Option<PruneConfig>,
    max_block: Option<BlockNumber>,
    static_file_producer: StaticFileProducer<ProviderFactory<N>>,
    executor: Executor,
    exex_manager_handle: ExExManagerHandle,
) -> eyre::Result<Pipeline<N>>
where
    N: ProviderNodeTypes,
    Client: BlockClient + 'static,
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
    )?;

    Ok(pipeline)
}

/// Builds the [Pipeline] with the given [`ProviderFactory`] and downloaders.
#[allow(clippy::too_many_arguments)]
pub fn build_pipeline<N, H, B, Executor>(
    provider_factory: ProviderFactory<N>,
    stage_config: &StageConfig,
    header_downloader: H,
    body_downloader: B,
    consensus: Arc<dyn Consensus>,
    max_block: Option<u64>,
    metrics_tx: reth_stages::MetricEventsSender,
    prune_config: Option<PruneConfig>,
    static_file_producer: StaticFileProducer<ProviderFactory<N>>,
    executor: Executor,
    exex_manager_handle: ExExManagerHandle,
) -> eyre::Result<Pipeline<N>>
where
    N: ProviderNodeTypes,
    H: HeaderDownloader + 'static,
    B: BodyDownloader + 'static,
    Executor: BlockExecutorProvider,
{
    let mut builder = Pipeline::<N>::builder();

    if let Some(max_block) = max_block {
        debug!(target: "reth::cli", max_block, "Configuring builder to use max block");
        builder = builder.with_max_block(max_block)
    }

    let (tip_tx, tip_rx) = watch::channel(B256::ZERO);

    let prune_modes = prune_config.map(|prune| prune.segments).unwrap_or_default();

    let pipeline = builder
        .with_tip_sender(tip_tx)
        .with_metrics_tx(metrics_tx)
        .add_stages(
            DefaultStages::new(
                provider_factory.clone(),
                tip_rx,
                Arc::clone(&consensus),
                header_downloader,
                body_downloader,
                executor.clone(),
                stage_config.clone(),
                prune_modes.clone(),
            )
            .set(ExecutionStage::new(
                executor,
                stage_config.execution.into(),
                stage_config.execution_external_clean_threshold(),
                prune_modes,
                exex_manager_handle,
            )),
        )
        .build(provider_factory, static_file_producer);

    Ok(pipeline)
}
