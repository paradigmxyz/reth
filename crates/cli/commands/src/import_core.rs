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
use reth_stages::{prelude::*, ControlFlow, Pipeline, StageId, StageSet};
use reth_static_file::StaticFileProducer;
use std::{path::Path, sync::Arc};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

/// Configuration for importing blocks from RLP files.
#[derive(Debug, Clone, Default)]
pub struct ImportConfig {
    /// Disables stages that require state.
    pub no_state: bool,
    /// Chunk byte length to read from file.
    pub chunk_len: Option<u64>,
    /// If true, fail immediately when an invalid block is encountered.
    /// By default (false), the import stops at the last valid block and exits successfully.
    pub fail_on_invalid_block: bool,
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
    /// Whether the import was stopped due to an invalid block.
    pub stopped_on_invalid_block: bool,
    /// The block number that was invalid, if any.
    pub bad_block: Option<u64>,
    /// The last valid block number when stopped due to invalid block.
    pub last_valid_block: Option<u64>,
}

impl ImportResult {
    /// Returns true if all blocks and transactions were imported successfully.
    pub fn is_complete(&self) -> bool {
        self.total_decoded_blocks == self.total_imported_blocks &&
            self.total_decoded_txns == self.total_imported_txns
    }

    /// Returns true if the import was successful, considering stop-on-invalid-block mode.
    ///
    /// In stop-on-invalid-block mode, a partial import is considered successful if we
    /// stopped due to an invalid block (leaving the DB at the last valid block).
    pub fn is_successful(&self) -> bool {
        self.is_complete() || self.stopped_on_invalid_block
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
    consensus: Arc<impl FullConsensus<N::Primitives> + 'static>,
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

    let provider = provider_factory.provider()?;
    let init_blocks = provider.tx_ref().entries::<tables::HeaderNumbers>()?;
    let init_txns = provider.tx_ref().entries::<tables::TransactionHashNumbers>()?;
    drop(provider);

    let mut total_decoded_blocks = 0;
    let mut total_decoded_txns = 0;

    let mut sealed_header = provider_factory
        .sealed_header(provider_factory.last_block_number()?)?
        .expect("should have genesis");

    let static_file_producer =
        StaticFileProducer::new(provider_factory.clone(), PruneModes::default());

    // Track if we stopped due to an invalid block
    let mut stopped_on_invalid_block = false;
    let mut bad_block_number: Option<u64> = None;
    let mut last_valid_block_number: Option<u64> = None;

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
            static_file_producer.clone(),
            import_config.no_state,
            executor.clone(),
        )?;

        // override the tip
        pipeline.set_tip(tip);
        debug!(target: "reth::import", ?tip, "Tip manually set");

        let latest_block_number =
            provider_factory.get_stage_checkpoint(StageId::Finish)?.map(|ch| ch.block_number);
        tokio::spawn(reth_node_events::node::handle_events(None, latest_block_number, events));

        // Run pipeline
        info!(target: "reth::import", "Starting sync pipeline");
        if import_config.fail_on_invalid_block {
            // Original behavior: fail on unwind
            tokio::select! {
                res = pipeline.run() => res?,
                _ = tokio::signal::ctrl_c() => {
                    info!(target: "reth::import", "Import interrupted by user");
                    break;
                },
            }
        } else {
            // Default behavior: Use run_loop() to handle unwinds gracefully
            let result = tokio::select! {
                res = pipeline.run_loop() => res,
                _ = tokio::signal::ctrl_c() => {
                    info!(target: "reth::import", "Import interrupted by user");
                    break;
                },
            };

            match result {
                Ok(ControlFlow::Unwind { target, bad_block }) => {
                    // An invalid block was encountered; stop at last valid block
                    let bad = bad_block.block.number;
                    warn!(
                        target: "reth::import",
                        bad_block = bad,
                        last_valid_block = target,
                        "Invalid block encountered during import; stopping at last valid block"
                    );
                    stopped_on_invalid_block = true;
                    bad_block_number = Some(bad);
                    last_valid_block_number = Some(target);
                    break;
                }
                Ok(ControlFlow::Continue { block_number }) => {
                    debug!(target: "reth::import", block_number, "Pipeline chunk completed");
                }
                Ok(ControlFlow::NoProgress { block_number }) => {
                    debug!(target: "reth::import", ?block_number, "Pipeline made no progress");
                }
                Err(e) => {
                    // Propagate other pipeline errors
                    return Err(e.into());
                }
            }
        }

        sealed_header = provider_factory
            .sealed_header(provider_factory.last_block_number()?)?
            .expect("should have genesis");
    }

    let provider = provider_factory.provider()?;
    let total_imported_blocks = provider.tx_ref().entries::<tables::HeaderNumbers>()? - init_blocks;
    let total_imported_txns =
        provider.tx_ref().entries::<tables::TransactionHashNumbers>()? - init_txns;

    let result = ImportResult {
        total_decoded_blocks,
        total_decoded_txns,
        total_imported_blocks,
        total_imported_txns,
        stopped_on_invalid_block,
        bad_block: bad_block_number,
        last_valid_block: last_valid_block_number,
    };

    if result.stopped_on_invalid_block {
        info!(target: "reth::import",
            total_imported_blocks,
            total_imported_txns,
            bad_block = ?result.bad_block,
            last_valid_block = ?result.last_valid_block,
            "Import stopped at last valid block due to invalid block"
        );
    } else if !result.is_complete() {
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
            "Chain was fully imported"
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
) -> eyre::Result<(Pipeline<N>, impl futures::Stream<Item = NodeEvent<N::Primitives>> + use<N, C, E>)>
where
    N: ProviderNodeTypes,
    C: FullConsensus<N::Primitives> + 'static,
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
