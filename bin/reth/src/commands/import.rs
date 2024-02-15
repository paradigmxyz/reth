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
use lightspeed_scheduler::job::Job;
use lightspeed_scheduler::scheduler::Scheduler;
use reth_beacon_consensus::BeaconConsensus;
use reth_config::Config;
use reth_db::database_metrics::DatabaseMetadata;

use reth_db::{database::Database, init_db, mdbx::DatabaseArguments};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder, file_client::FileClient,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_interfaces::consensus::Consensus;
use reth_node_core::{events::node::NodeEvent, init::init_genesis};
use reth_node_ethereum::EthEvmConfig;
use reth_primitives::{stage::StageId, ChainSpec, B256};
use reth_provider::{BlockNumReader, HeaderSyncMode, ProviderFactory, StageCheckpointReader};
use reth_stages::{
    prelude::*,
    stages::{ExecutionStage, ExecutionStageThresholds, SenderRecoveryStage, TotalDifficultyStage},
};
use std::time::Duration;
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

    /// The database configuration.
    #[clap(flatten)]
    db: DatabaseArgs,

    /// The URL to the remote source to import from.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    /// The end block to import.
    #[arg(long, value_name = "END_BLOCK", verbatim_doc_comment)]
    end_block: Option<u64>,

    /// Interval to import the block
    #[arg(long, value_name = "INTERVAL", verbatim_doc_comment, default_value = "30")]
    interval: u64,
}

impl ImportCommand {
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

        let interval = Duration::from_secs(self.interval);
        println!("interval {}", interval.as_secs());

        let job_executor = lightspeed_scheduler::JobExecutor::new_with_local_tz();

        job_executor
            .add_job_with_scheduler(
                Scheduler::Interval { interval_duration: interval, execute_at_startup: true },
                Job::new("import", "block importer", None, move || {
                    let import = self.clone();
                    let config = config.clone();
                    let provider_factory = provider_factory.clone();
                    let db = db.clone();
                    Box::pin(async move {
                        import.import(config, provider_factory, db.into()).await?;

                        Ok(())
                    })
                }),
            )
            .await;

        let job_executor_handler = job_executor.run().await?;

        match tokio::signal::ctrl_c().await {
            Ok(_) => {
                info!(target: "reth::cli", "Received SIGINT, shutting down");
            }
            Err(e) => {
                info!(target: "reth::cli", "Error while waiting for SIGINT: {:?}", e);
            }
        }

        job_executor_handler.await?;

        Ok(())
    }

    /// Import the chain from the file
    pub async fn import<DB>(
        &self,
        config: Config,
        provider_factory: ProviderFactory<DB>,
        db: Arc<DB>,
    ) -> eyre::Result<()>
    where
        DB: Database + DatabaseMetadata + Clone + Unpin + 'static,
    {
        let consensus = Arc::new(BeaconConsensus::new(self.chain.clone()));
        info!(target: "reth::cli", "Consensus engine initialized");

        // create a new FileClient
        info!(target: "reth::cli", "Importing chain file");

        // Get the local block number
        let start_block = provider_factory.provider()?.last_block_number()? + 1;

        info!(target: "reth::cli", "Starting block: {}", start_block);

        let file_client =
            Arc::new(FileClient::from_rpc_url(&self.rpc_url, start_block, self.end_block).await?);

        // override the tip
        let tip = file_client.remote_tip().expect("file client has no tip");
        info!(target: "reth::cli", "Chain file imported");

        let (mut pipeline, events) = self
            .build_import_pipeline(config, provider_factory.clone(), &consensus, file_client)
            .await?;

        // override the tip
        pipeline.set_tip(tip);
        debug!(target: "reth::cli", ?tip, "Tip manually set");

        let provider = provider_factory.provider()?;

        let latest_block_number =
            provider.get_stage_checkpoint(StageId::Finish)?.map(|ch| ch.block_number);
        tokio::spawn(reth_node_core::events::node::handle_events(
            None,
            latest_block_number,
            events,
            db.clone(),
        ));

        // Run pipeline
        info!(target: "reth::cli", "Starting sync pipeline");

        pipeline.run().await?;

        info!(target: "reth::cli", "Finishing up");
        Ok(())
    }

    async fn build_import_pipeline<DB, C>(
        &self,
        config: Config,
        provider_factory: ProviderFactory<DB>,
        consensus: &Arc<C>,
        file_client: Arc<FileClient>,
    ) -> eyre::Result<(Pipeline<DB>, impl Stream<Item = NodeEvent>)>
    where
        DB: Database + Clone + Unpin + 'static,
        C: Consensus + 'static,
    {
        if !file_client.has_canonical_blocks() {
            eyre::bail!("unable to import non canonical blocks");
        }

        let header_downloader = ReverseHeadersDownloaderBuilder::new(config.stages.headers)
            .build(file_client.clone(), consensus.clone())
            .into_task();

        let body_downloader = BodiesDownloaderBuilder::new(config.stages.bodies)
            .build(file_client.clone(), consensus.clone(), provider_factory.clone())
            .into_task();

        let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
        let factory =
            reth_revm::EvmProcessorFactory::new(self.chain.clone(), EthEvmConfig::default());

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
