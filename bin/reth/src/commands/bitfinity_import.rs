//! Command that initializes the node by importing a chain from a file.

use crate::{
    commands::import::load_config, dirs::DataDirPath, macros::block_executor, version::SHORT_VERSION
};
use futures::{Stream, StreamExt};
use lightspeed_scheduler::{job::Job, scheduler::Scheduler, JobExecutor};
use reth_beacon_consensus::BeaconConsensus;
use reth_config::{config::EtlConfig, Config};
use reth_db::{database_metrics::DatabaseMetadata, DatabaseEnv};

use reth_db::database::Database;
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder, bitfinity_evm_client::{CertificateCheckSettings, BitfinityEvmClient},
};
use reth_consensus::Consensus;
use reth_exex::ExExManagerHandle;
use reth_node_core::{args::BitfinityArgs, dirs::ChainPath};
use reth_node_events::node::NodeEvent;
use reth_primitives::{ChainSpec, PruneModes, B256};
use reth_provider::providers::BlockchainProvider;
use reth_provider::{BlockNumReader, CanonChainTracker, ChainSpecProvider, DatabaseProviderFactory, HeaderProvider, HeaderSyncMode, ProviderError, ProviderFactory, StaticFileProviderFactory};
use reth_stages::{
    prelude::*,
    stages::{ExecutionStage, ExecutionStageThresholds, SenderRecoveryStage},
    Pipeline, StageSet,
};
use reth_static_file::StaticFileProducer;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::watch;
use tracing::{debug, info};

/// Syncs RLP encoded blocks from a file.
#[derive(Clone)]
pub struct BitfinityImportCommand {
    /// The path to the configuration file to use.
    config: Option<PathBuf>,

    datadir: ChainPath<DataDirPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    chain: Arc<ChainSpec>,

    /// Bitfinity Related Args
    bitfinity: BitfinityArgs,

    provider_factory: ProviderFactory<Arc<DatabaseEnv>>,

    blockchain_provider: BlockchainProvider<Arc<DatabaseEnv>>,
}

/// Manually implement `Debug` for `ImportCommand` because BlockchainProvider doesn't implement it.
impl std::fmt::Debug for BitfinityImportCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportCommand")
            .field("config", &self.config)
            .field("datadir", &self.datadir)
            .field("chain", &self.chain)
            .field("bitfinity", &self.bitfinity)
            .finish()
    }
}

impl BitfinityImportCommand {

    /// Create a new `ImportCommand` with the given arguments.
    pub fn new(
        config: Option<PathBuf>,
        datadir: ChainPath<DataDirPath>,
        chain: Arc<ChainSpec>,
        bitfinity: BitfinityArgs,
        provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
        blockchain_provider: BlockchainProvider<Arc<DatabaseEnv>>,
    ) -> Self {
        Self { config, datadir, chain, bitfinity, provider_factory, blockchain_provider }
    }

    /// Execute `import` command
    pub async fn execute(self) -> eyre::Result<tokio::task::JoinHandle<()>> {
        info!(target: "reth::cli - BitfinityImportCommand", "reth {} starting", SHORT_VERSION);

        // add network name to data dir
        let config_path = self.config.clone().unwrap_or(self.datadir.config());
       
        let mut config: Config = load_config(config_path.clone())?;
        info!(target: "reth::cli - BitfinityImportCommand", path = ?config_path, "Configuration loaded");

        // Make sure ETL doesn't default to /tmp/, but to whatever datadir is set to
        if config.stages.etl.dir.is_none() {
            config.stages.etl.dir = Some(EtlConfig::from_datadir(self.datadir.data_dir()));
        }

        info!(target: "reth::cli - BitfinityImportCommand", "Database opened");

        let provider_factory = self.provider_factory.clone();

        let job_executor = JobExecutor::new_with_local_tz();
        
        // Schedule the import job
        {
            let interval = Duration::from_secs(self.bitfinity.import_interval);
            job_executor
            .add_job_with_scheduler(
                Scheduler::Interval { interval_duration: interval, execute_at_startup: true },
                Job::new("import", "block importer", None, move || {
                    let import = self.clone();
                    let config = config.clone();
                    let provider_factory = provider_factory.clone();
                    Box::pin(async move {
                        import.import(config, provider_factory).await?;
                        import.update_chain_info()?;
                        Ok(())
                    })
                }),
            )
            .await;
        }
        
        Ok(job_executor.run().await?)
    }

    /// Import the chain from the file
    async fn import<DB>(
        &self,
        config: Config,
        provider_factory: ProviderFactory<DB>,
    ) -> eyre::Result<()>
    where
        DB: Database + DatabaseMetadata + Clone + Unpin + 'static,
    {
        let consensus = Arc::new(BeaconConsensus::new(self.chain.clone()));
        debug!(target: "reth::cli - BitfinityImportCommand", "Consensus engine initialized");

        // Get the local block number
        let start_block = provider_factory.provider()?.last_block_number()? + 1;

        debug!(target: "reth::cli - BitfinityImportCommand", "Starting block: {}", start_block);

        let remote_client = Arc::new(
            BitfinityEvmClient::from_rpc_url(
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
            debug!(target: "reth::cli - BitfinityImportCommand", "No tip found, skipping import");
            return Ok(());
        };

        info!(target: "reth::cli - BitfinityImportCommand", "Chain blocks imported");

        let (mut pipeline, _events) = self.build_import_pipeline(
            &config,
            provider_factory.clone(),
            &consensus,
            remote_client,
            StaticFileProducer::new(
                provider_factory.clone(),
                provider_factory.static_file_provider(),
                PruneModes::default(),
            ),
        )?;

        // override the tip
        pipeline.set_tip(tip);
        debug!(target: "reth::cli - BitfinityImportCommand", ?tip, "Tip manually set");

        // Run pipeline
        debug!(target: "reth::cli - BitfinityImportCommand", "Starting sync pipeline");
        pipeline.run().await?;

        info!(target: "reth::cli - BitfinityImportCommand", "Finishing up");
        Ok(())
    }

    /// Update the chain info tracker with the latest header from the database.
    fn update_chain_info(&self) -> eyre::Result<()> {
        let provider = self.blockchain_provider.database_provider_ro()?;
        let chain_info = provider.chain_info()?;
        match provider.header_by_number(chain_info.best_number)? {
            Some(header) => {
                self.blockchain_provider.set_canonical_head(header.seal(chain_info.best_hash));
                Ok(())
            }
            None => Err(ProviderError::HeaderNotFound(chain_info.best_number.into()))?,
        }
    }


    fn build_import_pipeline<DB, C>(
        &self,
        config: &Config,
        provider_factory: ProviderFactory<DB>,
        consensus: &Arc<C>,
        remote_client: Arc<BitfinityEvmClient>,
        static_file_producer: StaticFileProducer<DB>,
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
        let executor = block_executor!(provider_factory.chain_spec());

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
                    config.prune.clone().map(|prune| prune.segments).unwrap_or_default(),
                    ExExManagerHandle::empty(),
                )),
            )
            .build(provider_factory, static_file_producer);

        let events = pipeline.events().map(Into::into);

        Ok((pipeline, events))
    }

}