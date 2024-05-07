//! Command that initializes the node by importing a chain from a file.

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
    macros::block_executor,
    version::SHORT_VERSION,
};
use clap::Parser;
use eyre::Context;
use futures::{Stream, StreamExt};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_config::{config::EtlConfig, Config};
use reth_consensus::Consensus;
use reth_db::{database::Database, init_db, tables, transaction::DbTx};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    file_client::{ChunkedFileReader, FileClient, DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE},
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_exex::ExExManagerHandle;
use reth_interfaces::p2p::{
    bodies::downloader::BodyDownloader,
    headers::downloader::{HeaderDownloader, SyncTarget},
};
use reth_node_core::init::init_genesis;
use reth_node_events::node::NodeEvent;
use reth_primitives::{stage::StageId, ChainSpec, PruneModes, B256};
use reth_provider::{
    BlockNumReader, ChainSpecProvider, HeaderProvider, HeaderSyncMode, ProviderError,
    ProviderFactory, StageCheckpointReader, StaticFileProviderFactory,
};
use reth_stages::{
    prelude::*,
    stages::{ExecutionStage, ExecutionStageThresholds, SenderRecoveryStage},
    Pipeline, StageSet,
};
use reth_static_file::StaticFileProducer;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::watch;
use tracing::{debug, error, info};

/// Syncs RLP encoded blocks from a file.
#[derive(Debug, Parser)]
pub struct ImportCommand {
    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    config: Option<PathBuf>,

    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    /// Disables stages that require state.
    #[arg(long, verbatim_doc_comment)]
    no_state: bool,

    /// Chunk byte length.
    #[arg(long, value_name = "CHUNK_LEN", verbatim_doc_comment)]
    chunk_len: Option<u64>,

    #[command(flatten)]
    db: DatabaseArgs,

    /// The path to a block file for import.
    ///
    /// The online stages (headers and bodies) are replaced by a file import, after which the
    /// remaining stages are executed.
    #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
    path: PathBuf,
}

impl ImportCommand {
    /// Execute `import` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        if self.no_state {
            info!(target: "reth::cli", "Disabled stages requiring state");
        }

        debug!(target: "reth::cli",
            chunk_byte_len=self.chunk_len.unwrap_or(DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE),
            "Chunking chain import"
        );

        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let config_path = self.config.clone().unwrap_or_else(|| data_dir.config());

        let mut config: Config = load_config(config_path.clone())?;
        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        // Make sure ETL doesn't default to /tmp/, but to whatever datadir is set to
        if config.stages.etl.dir.is_none() {
            config.stages.etl.dir = Some(EtlConfig::from_datadir(data_dir.data_dir()));
        }

        let db_path = data_dir.db();

        info!(target: "reth::cli", path = ?db_path, "Opening database");
        let db = Arc::new(init_db(db_path, self.db.database_args())?);
        info!(target: "reth::cli", "Database opened");
        let provider_factory =
            ProviderFactory::new(db.clone(), self.chain.clone(), data_dir.static_files())?;

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");

        init_genesis(provider_factory.clone())?;

        let consensus = Arc::new(EthBeaconConsensus::new(self.chain.clone()));
        info!(target: "reth::cli", "Consensus engine initialized");

        // open file
        let mut reader = ChunkedFileReader::new(&self.path, self.chunk_len).await?;

        let mut total_decoded_blocks = 0;
        let mut total_decoded_txns = 0;

        while let Some(file_client) = reader.next_chunk::<FileClient>().await? {
            // create a new FileClient from chunk read from file
            info!(target: "reth::cli",
                "Importing chain file chunk"
            );

            let tip = file_client.tip().ok_or(eyre::eyre!("file client has no tip"))?;
            info!(target: "reth::cli", "Chain file chunk read");

            total_decoded_blocks += file_client.headers_len();
            total_decoded_txns += file_client.total_transactions();

            let (mut pipeline, events) = build_import_pipeline(
                &config,
                provider_factory.clone(),
                &consensus,
                Arc::new(file_client),
                StaticFileProducer::new(
                    provider_factory.clone(),
                    provider_factory.static_file_provider(),
                    PruneModes::default(),
                ),
                self.no_state,
            )
            .await?;

            // override the tip
            pipeline.set_tip(tip);
            debug!(target: "reth::cli", ?tip, "Tip manually set");

            let provider = provider_factory.provider()?;

            let latest_block_number =
                provider.get_stage_checkpoint(StageId::Finish)?.map(|ch| ch.block_number);
            tokio::spawn(reth_node_events::node::handle_events(
                None,
                latest_block_number,
                events,
                db.clone(),
            ));

            // Run pipeline
            info!(target: "reth::cli", "Starting sync pipeline");
            tokio::select! {
                res = pipeline.run() => res?,
                _ = tokio::signal::ctrl_c() => {},
            }
        }

        let provider = provider_factory.provider()?;

        let total_imported_blocks = provider.tx_ref().entries::<tables::HeaderNumbers>()?;
        let total_imported_txns = provider.tx_ref().entries::<tables::TransactionHashNumbers>()?;

        if total_decoded_blocks != total_imported_blocks ||
            total_decoded_txns != total_imported_txns
        {
            error!(target: "reth::cli",
                total_decoded_blocks,
                total_imported_blocks,
                total_decoded_txns,
                total_imported_txns,
                "Chain was partially imported"
            );
        }

        info!(target: "reth::cli",
            total_imported_blocks,
            total_imported_txns,
            "Chain file imported"
        );

        Ok(())
    }
}

/// Builds import pipeline.
///
/// If configured to execute, all stages will run. Otherwise, only stages that don't require state
/// will run.
pub async fn build_import_pipeline<DB, C>(
    config: &Config,
    provider_factory: ProviderFactory<DB>,
    consensus: &Arc<C>,
    file_client: Arc<FileClient>,
    static_file_producer: StaticFileProducer<DB>,
    should_exec: bool,
) -> eyre::Result<(Pipeline<DB>, impl Stream<Item = NodeEvent>)>
where
    DB: Database + Clone + Unpin + 'static,
    C: Consensus + 'static,
{
    if !file_client.has_canonical_blocks() {
        eyre::bail!("unable to import non canonical blocks");
    }

    // Retrieve latest header found in the database.
    let last_block_number = provider_factory.last_block_number()?;
    let local_head = provider_factory
        .sealed_header(last_block_number)?
        .ok_or(ProviderError::HeaderNotFound(last_block_number.into()))?;

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
    let executor = block_executor!(provider_factory.chain_spec());

    let max_block = file_client.max_block().unwrap_or(0);

    let mut pipeline = Pipeline::builder()
        .with_tip_sender(tip_tx)
        // we want to sync all blocks the file client provides or 0 if empty
        .with_max_block(max_block)
        .add_stages(
            DefaultStages::new(
                provider_factory.clone(),
                HeaderSyncMode::Tip(tip_rx),
                consensus.clone(),
                header_downloader,
                body_downloader,
                executor.clone(),
                config.stages.etl.clone(),
            )
            .set(SenderRecoveryStage {
                commit_threshold: config.stages.sender_recovery.commit_threshold,
            })
            .set(ExecutionStage::new(
                executor,
                ExecutionStageThresholds {
                    max_blocks: config.stages.execution.max_blocks,
                    max_changes: config.stages.execution.max_changes,
                    max_cumulative_gas: config.stages.execution.max_cumulative_gas,
                    max_duration: config.stages.execution.max_duration,
                },
                config
                    .stages
                    .merkle
                    .clean_threshold
                    .max(config.stages.account_hashing.clean_threshold)
                    .max(config.stages.storage_hashing.clean_threshold),
                config.prune.as_ref().map(|prune| prune.segments.clone()).unwrap_or_default(),
                ExExManagerHandle::empty(),
            ))
            .disable_all_if(&StageId::STATE_REQUIRED, || should_exec),
        )
        .build(provider_factory, static_file_producer);

    let events = pipeline.events().map(Into::into);

    Ok((pipeline, events))
}

/// Loads the reth config
pub fn load_config(config_path: PathBuf) -> eyre::Result<Config> {
    confy::load_path::<Config>(config_path.clone())
        .wrap_err_with(|| format!("Could not load config file {config_path:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_common_import_command_chain_args() {
        for chain in SUPPORTED_CHAINS {
            let args: ImportCommand = ImportCommand::parse_from(["reth", "--chain", chain, "."]);
            assert_eq!(
                Ok(args.chain.chain),
                chain.parse::<reth_primitives::Chain>(),
                "failed to parse chain {chain}"
            );
        }
    }
}
