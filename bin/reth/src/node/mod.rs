//! Main node command
//!
//! Starts the client
use crate::{
    args::{NetworkArgs, RpcServerArgs},
    dirs::{ConfigPath, DbPath, PlatformPath},
    prometheus_exporter,
    runner::CliContext,
    utils::get_single_header,
};
use clap::{crate_version, Parser};
use events::NodeEvent;
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{pin_mut, stream::select as stream_select, Stream, StreamExt};
use reth_beacon_consensus::BeaconConsensus;
use reth_db::{
    database::Database,
    mdbx::{Env, WriteMap},
    tables,
    transaction::DbTx,
};
use reth_discv4::DEFAULT_DISCOVERY_PORT;
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_interfaces::{
    consensus::{Consensus, ForkchoiceState},
    p2p::{
        bodies::downloader::BodyDownloader,
        headers::{client::StatusUpdater, downloader::HeaderDownloader},
    },
    sync::SyncStateUpdater,
};
use reth_network::{
    error::NetworkError, FetchClient, NetworkConfig, NetworkHandle, NetworkManager,
};
use reth_network_api::NetworkInfo;
use reth_primitives::{BlockHashOrNumber, ChainSpec, Head, Header, SealedHeader, H256};
use reth_provider::{BlockProvider, HeaderProvider, ShareableDatabase};
use reth_rpc_engine_api::{EngineApi, EngineApiHandle};
use reth_staged_sync::{
    utils::{
        chainspec::genesis_value_parser,
        init::{init_db, init_genesis},
        parse_socket_address,
    },
    Config,
};
use reth_stages::{
    prelude::*,
    stages::{ExecutionStage, HeaderStage, SenderRecoveryStage, TotalDifficultyStage, FINISH},
};
use reth_tasks::TaskExecutor;
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{mpsc::unbounded_channel, watch};
use tracing::*;

pub mod events;

/// Start the node
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment, default_value_t)]
    config: PlatformPath<ConfigPath>,

    /// The path to the database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(long, value_name = "PATH", verbatim_doc_comment, default_value_t)]
    db: PlatformPath<DbPath>,

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

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long, value_name = "SOCKET", value_parser = parse_socket_address, help_heading = "Metrics")]
    metrics: Option<SocketAddr>,

    #[clap(flatten)]
    network: NetworkArgs,

    /// Prompt the downloader to download blocks one at a time.
    ///
    /// NOTE: This is for testing purposes only.
    #[arg(long = "debug.continuous", help_heading = "Debug")]
    continuous: bool,

    /// Set the chain tip manually for testing purposes.
    ///
    /// NOTE: This is a temporary flag
    #[arg(long = "debug.tip", help_heading = "Debug")]
    tip: Option<H256>,

    /// Runs the sync only up to the specified block
    #[arg(long = "debug.max-block", help_heading = "Debug")]
    max_block: Option<u64>,

    /// Flag indicating whether the node should be terminated after the pipeline sync.
    #[arg(long = "debug.terminate", help_heading = "Debug")]
    terminate: bool,

    #[clap(flatten)]
    rpc: RpcServerArgs,
}

impl Command {
    /// Execute `node` command
    // TODO: RPC
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", crate_version!());

        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit();

        let mut config: Config = self.load_config()?;
        info!(target: "reth::cli", path = %self.db, "Configuration loaded");

        info!(target: "reth::cli", path = %self.db, "Opening database");
        let db = Arc::new(init_db(&self.db)?);
        let shareable_db = ShareableDatabase::new(Arc::clone(&db), self.chain.clone());
        info!(target: "reth::cli", "Database opened");

        self.start_metrics_endpoint()?;

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");

        init_genesis(db.clone(), self.chain.clone())?;

        let (consensus, forkchoice_state_tx) = self.init_consensus()?;
        info!(target: "reth::cli", "Consensus engine initialized");

        self.init_trusted_nodes(&mut config);

        info!(target: "reth::cli", "Connecting to P2P network");
        let network_config =
            self.load_network_config(&config, Arc::clone(&db), ctx.task_executor.clone());
        let network = self.start_network(network_config, &ctx.task_executor, ()).await?;
        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), "Connected to P2P network");

        let test_transaction_pool = reth_transaction_pool::test_utils::testing_pool();
        info!(target: "reth::cli", "Test transaction pool initialized");

        let _rpc_server = self
            .rpc
            .start_rpc_server(
                shareable_db.clone(),
                test_transaction_pool.clone(),
                network.clone(),
                ctx.task_executor.clone(),
            )
            .await?;
        info!(target: "reth::cli", "Started RPC server");

        if self.continuous {
            info!(target: "reth::cli", "Continuous sync mode enabled");
        }

        let engine_api_handle =
            self.init_engine_api(Arc::clone(&db), forkchoice_state_tx, &ctx.task_executor);
        info!(target: "reth::cli", "Engine API handler initialized");

        let _auth_server = self
            .rpc
            .start_auth_server(
                shareable_db,
                test_transaction_pool,
                network.clone(),
                ctx.task_executor.clone(),
                engine_api_handle,
            )
            .await?;
        info!(target: "reth::cli", "Started Auth server");

        let (mut pipeline, events) = self
            .build_networked_pipeline(
                &mut config,
                network.clone(),
                &consensus,
                db.clone(),
                &ctx.task_executor,
            )
            .await?;

        ctx.task_executor.spawn(events::handle_events(Some(network.clone()), events));

        // Run pipeline
        let (rx, tx) = tokio::sync::oneshot::channel();
        info!(target: "reth::cli", "Starting sync pipeline");
        ctx.task_executor.spawn_critical_blocking("pipeline task", async move {
            let res = pipeline.run(db.clone()).await;
            let _ = rx.send(res);
        });

        tx.await??;

        info!(target: "reth::cli", "Pipeline has finished.");

        if self.terminate {
            Ok(())
        } else {
            // The pipeline has finished downloading blocks up to `--debug.tip` or
            // `--debug.max-block`. Keep other node components alive for further usage.
            futures::future::pending().await
        }
    }

    async fn build_networked_pipeline(
        &self,
        config: &mut Config,
        network: NetworkHandle,
        consensus: &Arc<dyn Consensus>,
        db: Arc<Env<WriteMap>>,
        task_executor: &TaskExecutor,
    ) -> eyre::Result<(Pipeline<Env<WriteMap>, impl SyncStateUpdater>, impl Stream<Item = NodeEvent>)>
    {
        let fetch_client = network.fetch_client().await?;
        let max_block = if let Some(block) = self.max_block {
            Some(block)
        } else if let Some(tip) = self.tip {
            Some(self.lookup_or_fetch_tip(db.clone(), fetch_client.clone(), tip).await?)
        } else {
            None
        };

        // TODO: remove Arc requirement from downloader builders.
        // building network downloaders using the fetch client
        let fetch_client = Arc::new(fetch_client);
        let header_downloader = ReverseHeadersDownloaderBuilder::from(config.stages.headers)
            .build(fetch_client.clone(), consensus.clone())
            .into_task_with(task_executor);

        let body_downloader = BodiesDownloaderBuilder::from(config.stages.bodies)
            .build(fetch_client.clone(), consensus.clone(), db.clone())
            .into_task_with(task_executor);

        let mut pipeline = self
            .build_pipeline(
                config,
                header_downloader,
                body_downloader,
                network.clone(),
                consensus,
                max_block,
                self.continuous,
            )
            .await?;

        let events = stream_select(
            network.event_listener().map(Into::into),
            pipeline.events().map(Into::into),
        );
        Ok((pipeline, events))
    }

    fn load_config(&self) -> eyre::Result<Config> {
        confy::load_path::<Config>(&self.config).wrap_err("Could not load config")
    }

    fn init_trusted_nodes(&self, config: &mut Config) {
        config.peers.connect_trusted_nodes_only = self.network.trusted_only;

        if !self.network.trusted_peers.is_empty() {
            info!(target: "reth::cli", "Adding trusted nodes");
            self.network.trusted_peers.iter().for_each(|peer| {
                config.peers.trusted_nodes.insert(*peer);
            });
        }
    }

    fn start_metrics_endpoint(&self) -> eyre::Result<()> {
        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", addr = %listen_addr, "Starting metrics endpoint");
            prometheus_exporter::initialize(listen_addr)
        } else {
            Ok(())
        }
    }

    fn init_consensus(&self) -> eyre::Result<(Arc<dyn Consensus>, watch::Sender<ForkchoiceState>)> {
        let (consensus, notifier) = BeaconConsensus::builder().build(self.chain.clone());

        if let Some(tip) = self.tip {
            debug!(target: "reth::cli", %tip, "Tip manually set");
            notifier.send(ForkchoiceState {
                head_block_hash: tip,
                safe_block_hash: tip,
                finalized_block_hash: tip,
            })?;
        } else {
            let warn_msg = "No tip specified. \
            reth cannot communicate with consensus clients, \
            so a tip must manually be provided for the online stages with --debug.tip <HASH>.";
            warn!(target: "reth::cli", warn_msg);
        }

        Ok((consensus, notifier))
    }

    fn init_engine_api(
        &self,
        db: Arc<Env<WriteMap>>,
        forkchoice_state_tx: watch::Sender<ForkchoiceState>,
        task_executor: &TaskExecutor,
    ) -> EngineApiHandle {
        let (message_tx, message_rx) = unbounded_channel();
        let engine_api = EngineApi::new(
            ShareableDatabase::new(db, self.chain.clone()),
            self.chain.clone(),
            message_rx,
            forkchoice_state_tx,
        );
        task_executor.spawn_critical("engine API task", engine_api);
        message_tx
    }

    /// Spawns the configured network and associated tasks and returns the [NetworkHandle] connected
    /// to that network.
    async fn start_network<C>(
        &self,
        config: NetworkConfig<C>,
        task_executor: &TaskExecutor,
        // TODO: integrate pool
        _pool: (),
    ) -> Result<NetworkHandle, NetworkError>
    where
        C: BlockProvider + HeaderProvider + Clone + Unpin + 'static,
    {
        let client = config.client.clone();
        let (handle, network, _txpool, eth) =
            NetworkManager::builder(config).await?.request_handler(client).split_with_handle();

        let known_peers_file = self.network.persistent_peers_file();
        task_executor.spawn_critical_with_signal("p2p network task", |shutdown| {
            run_network_until_shutdown(shutdown, network, known_peers_file)
        });

        task_executor.spawn_critical("p2p eth request handler", eth);

        // TODO spawn pool

        Ok(handle)
    }

    fn lookup_head(&self, db: Arc<Env<WriteMap>>) -> Result<Head, reth_interfaces::db::Error> {
        db.view(|tx| {
            let head = FINISH.get_progress(tx)?.unwrap_or_default();
            let header = tx
                .get::<tables::Headers>(head)?
                .expect("the header for the latest block is missing, database is corrupt");
            let total_difficulty = tx.get::<tables::HeaderTD>(head)?.expect(
                "the total difficulty for the latest block is missing, database is corrupt",
            );
            let hash = tx
                .get::<tables::CanonicalHeaders>(head)?
                .expect("the hash for the latest block is missing, database is corrupt");
            Ok::<Head, reth_interfaces::db::Error>(Head {
                number: head,
                hash,
                difficulty: header.difficulty,
                total_difficulty: total_difficulty.into(),
                timestamp: header.timestamp,
            })
        })?
        .map_err(Into::into)
    }

    /// Attempt to look up the block number for the tip hash in the database.
    /// If it doesn't exist, download the header and return the block number.
    ///
    /// NOTE: The download is attempted with infinite retries.
    async fn lookup_or_fetch_tip(
        &self,
        db: Arc<Env<WriteMap>>,
        fetch_client: FetchClient,
        tip: H256,
    ) -> Result<u64, reth_interfaces::Error> {
        Ok(self.fetch_tip(db, fetch_client, BlockHashOrNumber::Hash(tip)).await?.number)
    }

    /// Attempt to look up the block with the given number and return the header.
    ///
    /// NOTE: The download is attempted with infinite retries.
    async fn fetch_tip(
        &self,
        db: Arc<Env<WriteMap>>,
        fetch_client: FetchClient,
        tip: BlockHashOrNumber,
    ) -> Result<SealedHeader, reth_interfaces::Error> {
        let header = db.view(|tx| -> Result<Option<Header>, reth_db::Error> {
            let number = match tip {
                BlockHashOrNumber::Hash(hash) => tx.get::<tables::HeaderNumbers>(hash)?,
                BlockHashOrNumber::Number(number) => Some(number),
            };
            Ok(number.map(|number| tx.get::<tables::Headers>(number)).transpose()?.flatten())
        })??;

        // try to look up the header in the database
        if let Some(header) = header {
            info!(target: "reth::cli", ?tip, "Successfully looked up tip block in the database");
            return Ok(header.seal_slow())
        }

        info!(target: "reth::cli", ?tip, "Fetching tip block from the network.");
        loop {
            match get_single_header(fetch_client.clone(), tip).await {
                Ok(tip_header) => {
                    info!(target: "reth::cli", ?tip, "Successfully fetched tip");
                    return Ok(tip_header)
                }
                Err(error) => {
                    error!(target: "reth::cli", %error, "Failed to fetch the tip. Retrying...");
                }
            }
        }
    }

    fn load_network_config(
        &self,
        config: &Config,
        db: Arc<Env<WriteMap>>,
        executor: TaskExecutor,
    ) -> NetworkConfig<ShareableDatabase<Arc<Env<WriteMap>>>> {
        let head = self.lookup_head(Arc::clone(&db)).expect("the head block is missing");

        self.network
            .network_config(config, self.chain.clone())
            .with_task_executor(Box::new(executor))
            .set_head(head)
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                self.network.discovery.port.unwrap_or(DEFAULT_DISCOVERY_PORT),
            )))
            .build(ShareableDatabase::new(db, self.chain.clone()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_pipeline<H, B, U>(
        &self,
        config: &Config,
        header_downloader: H,
        body_downloader: B,
        updater: U,
        consensus: &Arc<dyn Consensus>,
        max_block: Option<u64>,
        continuous: bool,
    ) -> eyre::Result<Pipeline<Env<WriteMap>, U>>
    where
        H: HeaderDownloader + 'static,
        B: BodyDownloader + 'static,
        U: SyncStateUpdater + StatusUpdater + Clone + 'static,
    {
        let stage_conf = &config.stages;

        let mut builder = Pipeline::builder();

        if let Some(max_block) = max_block {
            debug!(target: "reth::cli", max_block, "Configuring builder to use max block");
            builder = builder.with_max_block(max_block)
        }

        let factory = reth_executor::Factory::new(self.chain.clone());

        let default_stages = if continuous {
            let continuous_headers =
                HeaderStage::new(header_downloader, consensus.clone()).continuous();
            let online_builder = OnlineStages::builder_with_headers(
                continuous_headers,
                consensus.clone(),
                body_downloader,
            );
            DefaultStages::<H, B, U, reth_executor::Factory>::add_offline_stages(
                online_builder,
                updater.clone(),
                factory.clone(),
            )
        } else {
            DefaultStages::new(
                consensus.clone(),
                header_downloader,
                body_downloader,
                updater.clone(),
                factory.clone(),
            )
            .builder()
        };

        let pipeline = builder
            .with_sync_state_updater(updater)
            .add_stages(
                default_stages
                    .set(
                        TotalDifficultyStage::new(consensus.clone())
                            .with_commit_threshold(stage_conf.total_difficulty.commit_threshold),
                    )
                    .set(SenderRecoveryStage {
                        commit_threshold: stage_conf.sender_recovery.commit_threshold,
                    })
                    .set(ExecutionStage::new(factory, stage_conf.execution.commit_threshold)),
            )
            .build();

        Ok(pipeline)
    }
}

/// Drives the [NetworkManager] future until a [Shutdown](reth_tasks::shutdown::Shutdown) signal is
/// received. If configured, this writes known peers to `persistent_peers_file` afterwards.
async fn run_network_until_shutdown<C>(
    shutdown: reth_tasks::shutdown::Shutdown,
    network: NetworkManager<C>,
    persistent_peers_file: Option<PathBuf>,
) where
    C: BlockProvider + HeaderProvider + Clone + Unpin + 'static,
{
    pin_mut!(network, shutdown);

    tokio::select! {
        _ = &mut network => {},
        _ = shutdown => {},
    }

    if let Some(file_path) = persistent_peers_file {
        let known_peers = network.all_peers().collect::<Vec<_>>();
        if let Ok(known_peers) = serde_json::to_string_pretty(&known_peers) {
            trace!(target : "reth::cli", peers_file =?file_path, num_peers=%known_peers.len(), "Saving current peers");
            let parent_dir = file_path.parent().map(std::fs::create_dir_all).transpose();
            match parent_dir.and_then(|_| std::fs::write(&file_path, known_peers)) {
                Ok(_) => {
                    info!(target: "reth::cli", peers_file=?file_path, "Wrote network peers to file");
                }
                Err(err) => {
                    warn!(target: "reth::cli", ?err, peers_file=?file_path, "Failed to write network peers to file");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_help_node_command() {
        let err = Command::try_parse_from(["reth", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    #[test]
    fn parse_common_node_command_chain_args() {
        for chain in ["mainnet", "sepolia", "goerli"] {
            let args: Command = Command::parse_from(["reth", "--chain", chain]);
            assert_eq!(args.chain.chain, chain.parse().unwrap());
        }
    }
}
