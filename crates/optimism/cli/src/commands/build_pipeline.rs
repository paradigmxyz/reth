use alloy_primitives::B256;
use futures_util::{Stream, StreamExt};
use reth_config::Config;
use reth_consensus::Consensus;
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder, file_client::FileClient,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_errors::ProviderError;
use reth_network_p2p::{
    bodies::downloader::BodyDownloader,
    headers::downloader::{HeaderDownloader, SyncTarget},
};
use reth_node_builder::NodeTypesWithDB;
use reth_node_events::node::NodeEvent;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_evm::OpExecutorProvider;
use reth_provider::{BlockNumReader, ChainSpecProvider, HeaderProvider, ProviderFactory};
use reth_prune::PruneModes;
use reth_stages::{sets::DefaultStages, Pipeline, StageSet};
use reth_stages_types::StageId;
use reth_static_file::StaticFileProducer;
use std::sync::Arc;
use tokio::sync::watch;

/// Builds import pipeline.
///
/// If configured to execute, all stages will run. Otherwise, only stages that don't require state
/// will run.
pub(crate) async fn build_import_pipeline<N, C>(
    config: &Config,
    provider_factory: ProviderFactory<N>,
    consensus: &Arc<C>,
    file_client: Arc<FileClient>,
    static_file_producer: StaticFileProducer<ProviderFactory<N>>,
    disable_exec: bool,
) -> eyre::Result<(Pipeline<N>, impl Stream<Item = NodeEvent>)>
where
    N: NodeTypesWithDB<ChainSpec = OpChainSpec>,
    C: Consensus + 'static,
{
    if !file_client.has_canonical_blocks() {
        eyre::bail!("unable to import non canonical blocks");
    }

    // Retrieve latest header found in the database.
    let last_block_number = provider_factory.last_block_number()?;
    let local_head = provider_factory
        .sealed_header(last_block_number)?
        .ok_or_else(|| ProviderError::HeaderNotFound(last_block_number.into()))?;

    let mut header_downloader = ReverseHeadersDownloaderBuilder::new(config.stages.headers)
        .build(file_client.clone(), consensus.clone())
        .into_task();
    // TODO: The pipeline should correctly configure the downloader on its own.
    // Find the possibility to remove unnecessary pre-configuration.
    header_downloader.update_local_head(local_head);
    header_downloader.update_sync_target(SyncTarget::Tip(file_client.tip().unwrap()));

    let mut body_downloader = BodiesDownloaderBuilder::new(config.stages.bodies)
        .build(file_client.clone(), consensus.clone(), provider_factory.clone())
        .into_task();
    // TODO: The pipeline should correctly configure the downloader on its own.
    // Find the possibility to remove unnecessary pre-configuration.
    body_downloader
        .set_download_range(file_client.min_block().unwrap()..=file_client.max_block().unwrap())
        .expect("failed to set download range");

    let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
    let executor = OpExecutorProvider::optimism(provider_factory.chain_spec());

    let max_block = file_client.max_block().unwrap_or(0);

    let pipeline = Pipeline::<N>::builder()
        .with_tip_sender(tip_tx)
        // we want to sync all blocks the file client provides or 0 if empty
        .with_max_block(max_block)
        .add_stages(
            DefaultStages::new(
                provider_factory.clone(),
                tip_rx,
                consensus.clone(),
                header_downloader,
                body_downloader,
                executor,
                config.stages.clone(),
                PruneModes::default(),
            )
            .builder()
            .disable_all_if(&StageId::STATE_REQUIRED, || disable_exec),
        )
        .build(provider_factory, static_file_producer);

    let events = pipeline.events().map(Into::into);

    Ok((pipeline, events))
}
