//! Command that initializes the node by importing a chain from a file.

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
    version::SHORT_VERSION,
};
use clap::Parser;
use eyre::Context;
use futures::{Stream, StreamExt};
use reth_beacon_consensus::BeaconConsensus;
use reth_config::Config;
use reth_db::database_metrics::DatabaseMetadata;

use reth_db::{database::Database, init_db, mdbx::DatabaseArguments};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder, remote_client::{CertificateCheckSettings, RemoteClient},
};
use reth_interfaces::consensus::Consensus;
use reth_node_core::{args::BitfinityArgs, events::node::NodeEvent, init::init_genesis};
use reth_node_ethereum::EthEvmConfig;
use reth_primitives::{ChainSpec, B256};
use reth_provider::{BlockNumReader, HeaderSyncMode, ProviderFactory};
use reth_stages::{
    prelude::*,
    stages::{ExecutionStage, ExecutionStageThresholds, SenderRecoveryStage, TotalDifficultyStage},
};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::watch;
use tracing::{debug, info};

/// Syncs RLP encoded blocks from a file.
#[derive(Debug, Parser, Clone)]
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

    /// Bitfinity Related Args
    #[clap(flatten)]
    bitfinity: BitfinityArgs,

    /// The database configuration.
    #[clap(flatten)]
    db: DatabaseArgs,
}

impl ImportCommand {
    /// Create a new `ImportCommand` with the given arguments.
    pub fn new(
        config: Option<PathBuf>,
        datadir: MaybePlatformPath<DataDirPath>,
        chain: Arc<ChainSpec>,
        bitfinity: BitfinityArgs,
        db: DatabaseArgs,
    ) -> Self {
        Self { config, datadir, chain, bitfinity, db }
    }

    /// Execute `import` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let config_path = self.config.clone().unwrap_or(data_dir.config_path());
        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);

        let config: Config = self.load_config(config_path.clone())?;
        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        let db_path = data_dir.db_path();

        info!(target: "reth::cli", path = ?db_path, "Opening database");
        let db =
            Arc::new(init_db(&db_path, DatabaseArguments::default().log_level(self.db.log_level))?);

        info!(target: "reth::cli", "Database opened");

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");

        init_genesis(db.clone(), self.chain.clone())?;

        let provider_factory = ProviderFactory::new(db.clone(), self.chain.clone());

        self.import(config, provider_factory, db.into()).await?;

        Ok(())
    }

    /// Import the chain from the file
    pub async fn import<DB>(
        &self,
        config: Config,
        provider_factory: ProviderFactory<DB>,
        _db: Arc<DB>,
    ) -> eyre::Result<()>
    where
        DB: Database + DatabaseMetadata + Clone + Unpin + 'static,
    {
        let consensus = Arc::new(BeaconConsensus::new(self.chain.clone()));
        debug!(target: "reth::cli", "Consensus engine initialized");

        // Get the local block number
        let start_block = provider_factory.provider()?.last_block_number()? + 1;

        debug!(target: "reth::cli", "Starting block: {}", start_block);

        let remote_client = Arc::new(
            RemoteClient::from_rpc_url(
                &self.bitfinity.rpc_url,
                start_block,
                self.bitfinity.end_block,
                self.bitfinity.batch_size,
                Some(CertificateCheckSettings{
                    evmc_principal: self.bitfinity.evmc_principal.clone(),
                    ic_root_key: self.bitfinity.ic_root_key.clone(),
                }),
            )
            .await?,
        );

        // override the tip
        let tip = if let Some(tip) = remote_client.tip() {
            tip
        } else {
            debug!(target: "reth::cli", "No tip found, skipping import");
            return Ok(());
        };

        info!(target: "reth::cli", "Chain blocks imported");

        let (mut pipeline, _events) = self.build_import_pipeline(
            config,
            provider_factory.clone(),
            &consensus,
            remote_client,
        )?;

        // override the tip
        pipeline.set_tip(tip);
        debug!(target: "reth::cli", ?tip, "Tip manually set");

        // let provider = provider_factory.provider()?;

        // let latest_block_number =
        //     provider.get_stage_checkpoint(StageId::Finish)?.map(|ch| ch.block_number);
        // tokio::spawn(reth_node_core::events::node::handle_events(
        //     None,
        //     latest_block_number,
        //     events,
        //     db.clone(),
        // ));

        // Run pipeline
        debug!(target: "reth::cli", "Starting sync pipeline");
        pipeline.run().await?;

        info!(target: "reth::cli", "Finishing up");
        Ok(())
    }

    fn build_import_pipeline<DB, C>(
        &self,
        config: Config,
        provider_factory: ProviderFactory<DB>,
        consensus: &Arc<C>,
        remote_client: Arc<RemoteClient>,
    ) -> eyre::Result<(Pipeline<DB>, impl Stream<Item = NodeEvent>)>
    where
        DB: Database + Clone + Unpin + 'static,
        C: Consensus + 'static,
    {
        if !remote_client.has_canonical_blocks() {
            eyre::bail!("unable to import non canonical blocks");
        }

        let header_downloader = ReverseHeadersDownloaderBuilder::new(config.stages.headers)
            .build(remote_client.clone(), consensus.clone())
            .into_task();

        let body_downloader = BodiesDownloaderBuilder::new(config.stages.bodies)
            .build(remote_client.clone(), consensus.clone(), provider_factory.clone())
            .into_task();

        let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
        let factory =
            reth_revm::EvmProcessorFactory::new(self.chain.clone(), EthEvmConfig::default());

        let max_block = remote_client.max_block().unwrap_or(0);
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
                    factory.clone(),
                )
                .set(
                    TotalDifficultyStage::new(consensus.clone())
                        .with_commit_threshold(config.stages.total_difficulty.commit_threshold),
                )
                .set(SenderRecoveryStage {
                    commit_threshold: config.stages.sender_recovery.commit_threshold,
                })
                .set(ExecutionStage::new(
                    factory,
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
                    config.prune.map(|prune| prune.segments).unwrap_or_default(),
                )),
            )
            .build(provider_factory);

        let events = pipeline.events().map(Into::into);

        Ok((pipeline, events))
    }

    /// Loads the reth config
    fn load_config(&self, config_path: PathBuf) -> eyre::Result<Config> {
        confy::load_path::<Config>(config_path.clone())
            .wrap_err_with(|| format!("Could not load config file {:?}", config_path))
    }
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
