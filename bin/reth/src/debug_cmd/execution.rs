//! Command for debugging execution.
use crate::{
    args::{get_secret_key, utils::genesis_value_parser, NetworkArgs},
    dirs::{DataDirPath, MaybePlatformPath},
    node::events,
    runner::CliContext,
    utils::get_single_header,
};
use clap::Parser;
use futures::{stream::select as stream_select, StreamExt};
use reth_beacon_consensus::BeaconConsensus;
use reth_config::Config;
use reth_db::{database::Database, init_db, DatabaseEnv};
use reth_discv4::DEFAULT_DISCOVERY_PORT;
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_interfaces::{
    consensus::Consensus,
    p2p::{bodies::client::BodiesClient, headers::client::HeadersClient},
};
use reth_network::NetworkHandle;
use reth_network_api::NetworkInfo;
use reth_primitives::{stage::StageId, BlockHashOrNumber, BlockNumber, ChainSpec, H256};
use reth_provider::{BlockExecutionWriter, ProviderFactory, StageCheckpointReader};
use reth_staged_sync::utils::init::init_genesis;
use reth_stages::{
    sets::DefaultStages,
    stages::{
        ExecutionStage, ExecutionStageThresholds, HeaderSyncMode, SenderRecoveryStage,
        TotalDifficultyStage,
    },
    Pipeline, PipelineError, StageSet,
};
use reth_tasks::TaskExecutor;
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::watch;
use tracing::*;

/// `reth execution-debug` command
#[derive(Debug, Parser)]
pub struct Command {
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
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    #[clap(flatten)]
    network: NetworkArgs,

    /// Set the chain tip manually for testing purposes.
    ///
    /// NOTE: This is a temporary flag
    #[arg(long = "debug.tip", help_heading = "Debug")]
    pub tip: Option<H256>,

    /// The maximum block height.
    #[arg(long)]
    pub to: u64,

    /// The block interval for sync and unwind.
    /// Defaults to `1000`.
    #[arg(long, default_value = "1000")]
    pub interval: u64,
}

impl Command {
    fn build_pipeline<DB, Client>(
        &self,
        config: &Config,
        client: Client,
        consensus: Arc<dyn Consensus>,
        db: DB,
        task_executor: &TaskExecutor,
    ) -> eyre::Result<Pipeline<DB>>
    where
        DB: Database + Unpin + Clone + 'static,
        Client: HeadersClient + BodiesClient + Clone + 'static,
    {
        // building network downloaders using the fetch client
        let header_downloader = ReverseHeadersDownloaderBuilder::from(config.stages.headers)
            .build(client.clone(), Arc::clone(&consensus))
            .into_task_with(task_executor);

        let body_downloader = BodiesDownloaderBuilder::from(config.stages.bodies)
            .build(client, Arc::clone(&consensus), db.clone())
            .into_task_with(task_executor);

        let stage_conf = &config.stages;

        let (tip_tx, tip_rx) = watch::channel(H256::zero());
        let factory = reth_revm::Factory::new(self.chain.clone());

        let header_mode = HeaderSyncMode::Tip(tip_rx);
        let pipeline = Pipeline::builder()
            .with_tip_sender(tip_tx)
            .add_stages(
                DefaultStages::new(
                    header_mode,
                    Arc::clone(&consensus),
                    header_downloader,
                    body_downloader,
                    factory.clone(),
                )
                .set(
                    TotalDifficultyStage::new(consensus)
                        .with_commit_threshold(stage_conf.total_difficulty.commit_threshold),
                )
                .set(SenderRecoveryStage {
                    commit_threshold: stage_conf.sender_recovery.commit_threshold,
                })
                .set(ExecutionStage::new(
                    factory,
                    ExecutionStageThresholds { max_blocks: None, max_changes: None },
                )),
            )
            .build(db, self.chain.clone());

        Ok(pipeline)
    }

    async fn build_network(
        &self,
        config: &Config,
        task_executor: TaskExecutor,
        db: Arc<DatabaseEnv>,
        network_secret_path: PathBuf,
        default_peers_path: PathBuf,
    ) -> eyre::Result<NetworkHandle> {
        let secret_key = get_secret_key(&network_secret_path)?;
        let network = self
            .network
            .network_config(config, self.chain.clone(), secret_key, default_peers_path)
            .with_task_executor(Box::new(task_executor))
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                self.network.port.unwrap_or(DEFAULT_DISCOVERY_PORT),
            )))
            .discovery_addr(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                self.network.discovery.port.unwrap_or(DEFAULT_DISCOVERY_PORT),
            )))
            .build(ProviderFactory::new(db, self.chain.clone()))
            .start_network()
            .await?;
        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        Ok(network)
    }

    async fn fetch_block_hash<Client: HeadersClient>(
        &self,
        client: Client,
        block: BlockNumber,
    ) -> eyre::Result<H256> {
        info!(target: "reth::cli", ?block, "Fetching block from the network.");
        loop {
            match get_single_header(&client, BlockHashOrNumber::Number(block)).await {
                Ok(tip_header) => {
                    info!(target: "reth::cli", ?block, "Successfully fetched block");
                    return Ok(tip_header.hash)
                }
                Err(error) => {
                    error!(target: "reth::cli", ?block, %error, "Failed to fetch the block. Retrying...");
                }
            }
        }
    }

    /// Execute `execution-debug` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        let config = Config::default();

        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        std::fs::create_dir_all(&db_path)?;
        let db = Arc::new(init_db(db_path)?);

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");
        init_genesis(db.clone(), self.chain.clone())?;

        let consensus: Arc<dyn Consensus> = Arc::new(BeaconConsensus::new(Arc::clone(&self.chain)));

        // Configure and build network
        let network_secret_path =
            self.network.p2p_secret_key.clone().unwrap_or_else(|| data_dir.p2p_secret_path());
        let network = self
            .build_network(
                &config,
                ctx.task_executor.clone(),
                db.clone(),
                network_secret_path,
                data_dir.known_peers_path(),
            )
            .await?;

        // Configure the pipeline
        let fetch_client = network.fetch_client().await?;
        let mut pipeline = self.build_pipeline(
            &config,
            fetch_client.clone(),
            Arc::clone(&consensus),
            db.clone(),
            &ctx.task_executor,
        )?;

        let factory = ProviderFactory::new(&db, self.chain.clone());
        let provider = factory.provider().map_err(PipelineError::Interface)?;

        let latest_block_number =
            provider.get_stage_checkpoint(StageId::Finish)?.map(|ch| ch.block_number);
        if latest_block_number.unwrap_or_default() >= self.to {
            info!(target: "reth::cli", latest = latest_block_number, "Nothing to run");
            return Ok(())
        }

        let pipeline_events = pipeline.events();
        let events = stream_select(
            network.event_listener().map(Into::into),
            pipeline_events.map(Into::into),
        );
        ctx.task_executor.spawn_critical(
            "events task",
            events::handle_events(Some(network.clone()), latest_block_number, events),
        );

        let mut current_max_block = latest_block_number.unwrap_or_default();
        while current_max_block < self.to {
            let next_block = current_max_block + 1;
            let target_block = self.to.min(current_max_block + self.interval);
            let target_block_hash =
                self.fetch_block_hash(fetch_client.clone(), target_block).await?;

            // Run the pipeline
            info!(target: "reth::cli", from = next_block, to = target_block, tip = ?target_block_hash, "Starting pipeline");
            pipeline.set_tip(target_block_hash);
            let result = pipeline.run_loop().await?;
            trace!(target: "reth::cli", from = next_block, to = target_block, tip = ?target_block_hash, ?result, "Pipeline finished");

            // Unwind the pipeline without committing.
            {
                factory
                    .provider_rw()
                    .map_err(PipelineError::Interface)?
                    .take_block_and_execution_range(&self.chain, next_block..=target_block)?;
            }

            // Update latest block
            current_max_block = target_block;
        }

        Ok(())
    }
}
