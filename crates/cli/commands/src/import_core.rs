//! Core import functionality without CLI dependencies.

use alloy_primitives::B256;
use futures::StreamExt;
use reth_config::Config;
use reth_consensus::FullConsensus;
use reth_db_api::{tables, transaction::DbTx};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    file_client::{ChunkedFileReader, FileClient, DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE},
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_evm::ConfigureEvm;
use reth_network_p2p::{
    bodies::downloader::BodyDownloader,
    headers::downloader::{HeaderDownloader, SyncTarget},
};
use reth_node_api::BlockTy;
use reth_node_events::node::NodeEvent;
use reth_provider::{
    providers::ProviderNodeTypes, BlockNumReader, HeaderProvider, ProviderError, ProviderFactory,
    StageCheckpointReader,
};
use reth_prune::PruneModes;
use reth_stages::{prelude::*, Pipeline, StageId, StageSet};
use reth_static_file::StaticFileProducer;
use std::{path::Path, sync::Arc};
use tokio::sync::watch;
use tracing::{debug, error, info};

/// Configuration for importing blocks from RLP files.
#[derive(Debug, Clone, Default)]
pub struct ImportConfig {
    /// Disables stages that require state.
    pub no_state: bool,
    /// Chunk byte length to read from file.
    pub chunk_len: Option<u64>,
}

/// Result of an import operation.
#[derive(Debug)]
pub struct ImportResult {
    /// Total number of blocks decoded from the file.
    pub total_decoded_blocks: usize,
    /// Total number of transactions decoded from the file.
    pub total_decoded_txns: usize,
    /// Total number of blocks imported into the database.
    pub total_imported_blocks: usize,
    /// Total number of transactions imported into the database.
    pub total_imported_txns: usize,
}

impl ImportResult {
    /// Returns true if all blocks and transactions were imported successfully.
    pub fn is_complete(&self) -> bool {
        self.total_decoded_blocks == self.total_imported_blocks &&
            self.total_decoded_txns == self.total_imported_txns
    }
}

/// Imports blocks from an RLP-encoded file into the database.
///
/// This function reads RLP-encoded blocks from a file in chunks and imports them
/// using the pipeline infrastructure. It's designed to be used both from the CLI
/// and from test code.
pub async fn import_blocks_from_file<N>(
    path: &Path,
    import_config: ImportConfig,
    provider_factory: ProviderFactory<N>,
    config: &Config,
    executor: impl ConfigureEvm<Primitives = N::Primitives> + 'static,
    consensus: Arc<
        impl FullConsensus<N::Primitives, Error = reth_consensus::ConsensusError> + 'static,
    >,
) -> eyre::Result<ImportResult>
where
    N: ProviderNodeTypes,
{
    if import_config.no_state {
        info!(target: "reth::import", "Disabled stages requiring state");
    }

    debug!(target: "reth::import",
        chunk_byte_len=import_config.chunk_len.unwrap_or(DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE),
        "Chunking chain import"
    );

    info!(target: "reth::import", "Consensus engine initialized");

    // open file
    let mut reader = ChunkedFileReader::new(path, import_config.chunk_len).await?;

    let mut total_decoded_blocks = 0;
    let mut total_decoded_txns = 0;

    let mut sealed_header = provider_factory
        .sealed_header(provider_factory.last_block_number()?)?
        .expect("should have genesis");

    while let Some(file_client) =
        reader.next_chunk::<BlockTy<N>>(consensus.clone(), Some(sealed_header)).await?
    {
        // create a new FileClient from chunk read from file
        info!(target: "reth::import",
            "Importing chain file chunk"
        );

        let tip = file_client.tip().ok_or(eyre::eyre!("file client has no tip"))?;
        info!(target: "reth::import", "Chain file chunk read");

        total_decoded_blocks += file_client.headers_len();
        total_decoded_txns += file_client.total_transactions();

        let (mut pipeline, events) = build_import_pipeline_impl(
            config,
            provider_factory.clone(),
            &consensus,
            Arc::new(file_client),
            StaticFileProducer::new(provider_factory.clone(), PruneModes::default()),
            import_config.no_state,
            executor.clone(),
        )?;

        // override the tip
        pipeline.set_tip(tip);
        debug!(target: "reth::import", ?tip, "Tip manually set");

        let provider = provider_factory.provider()?;

        let latest_block_number =
            provider.get_stage_checkpoint(StageId::Finish)?.map(|ch| ch.block_number);
        tokio::spawn(reth_node_events::node::handle_events(None, latest_block_number, events));

        // Run pipeline
        info!(target: "reth::import", "Starting sync pipeline");
        tokio::select! {
            res = pipeline.run() => res?,
            _ = tokio::signal::ctrl_c() => {
                info!(target: "reth::import", "Import interrupted by user");
                break;
            },
        }

        sealed_header = provider_factory
            .sealed_header(provider_factory.last_block_number()?)?
            .expect("should have genesis");
    }

    let provider = provider_factory.provider()?;

    let total_imported_blocks = provider.tx_ref().entries::<tables::HeaderNumbers>()?;
    let total_imported_txns = provider.tx_ref().entries::<tables::TransactionHashNumbers>()?;

    let result = ImportResult {
        total_decoded_blocks,
        total_decoded_txns,
        total_imported_blocks,
        total_imported_txns,
    };

    if !result.is_complete() {
        error!(target: "reth::import",
            total_decoded_blocks,
            total_imported_blocks,
            total_decoded_txns,
            total_imported_txns,
            "Chain was partially imported"
        );
    } else {
        info!(target: "reth::import",
            total_imported_blocks,
            total_imported_txns,
            "Chain file imported"
        );
    }

    Ok(result)
}

/// Builds import pipeline.
///
/// If configured to execute, all stages will run. Otherwise, only stages that don't require state
/// will run.
pub fn build_import_pipeline_impl<N, C, E>(
    config: &Config,
    provider_factory: ProviderFactory<N>,
    consensus: &Arc<C>,
    file_client: Arc<FileClient<BlockTy<N>>>,
    static_file_producer: StaticFileProducer<ProviderFactory<N>>,
    disable_exec: bool,
    evm_config: E,
) -> eyre::Result<(Pipeline<N>, impl futures::Stream<Item = NodeEvent<N::Primitives>>)>
where
    N: ProviderNodeTypes,
    C: FullConsensus<N::Primitives, Error = reth_consensus::ConsensusError> + 'static,
    E: ConfigureEvm<Primitives = N::Primitives> + 'static,
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

    let max_block = file_client.max_block().unwrap_or(0);

    let pipeline = Pipeline::builder()
        .with_tip_sender(tip_tx)
        // we want to sync all blocks the file client provides or 0 if empty
        .with_max_block(max_block)
        .with_fail_on_unwind(true)
        .add_stages(
            DefaultStages::new(
                provider_factory.clone(),
                tip_rx,
                consensus.clone(),
                header_downloader,
                body_downloader,
                evm_config,
                config.stages.clone(),
                PruneModes::default(),
                None,
            )
            .builder()
            .disable_all_if(&StageId::STATE_REQUIRED, || disable_exec),
        )
        .build(provider_factory, static_file_producer);

    let events = pipeline.events().map(Into::into);

    Ok((pipeline, events))
}
