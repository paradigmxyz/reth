use crate::{
    args::{
        get_secret_key,
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs, NetworkArgs,
    },
    commands::debug_cmd::engine_api_store::{EngineApiStore, StoredEngineApiMessage},
    core::cli::runner::CliContext,
    dirs::{DataDirPath, MaybePlatformPath},
};
use clap::Parser;
use eyre::Context;
use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_beacon_consensus::{hooks::EngineHooks, BeaconConsensus, BeaconConsensusEngine};
use reth_blockchain_tree::{
    BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals,
};
use reth_config::Config;
use reth_db::{init_db, mdbx::DatabaseArguments, DatabaseEnv};
use reth_interfaces::consensus::Consensus;
use reth_network::NetworkHandle;
use reth_network_api::NetworkInfo;
#[cfg(not(feature = "optimism"))]
use reth_node_ethereum::{EthEngineTypes, EthEvmConfig};
#[cfg(feature = "optimism")]
use reth_node_optimism::{OptimismEngineTypes, OptimismEvmConfig};
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
use reth_primitives::{fs, ChainSpec};
use reth_provider::{providers::BlockchainProvider, CanonStateSubscriptions, ProviderFactory};
use reth_revm::EvmProcessorFactory;
use reth_stages::Pipeline;
use reth_tasks::TaskExecutor;
use reth_transaction_pool::noop::NoopTransactionPool;
use std::{
    net::{SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use tracing::*;

/// `reth debug replay-engine` command
/// This script will read stored engine API messages and replay them by the timestamp.
/// It does not require
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
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    #[clap(flatten)]
    db: DatabaseArgs,

    #[clap(flatten)]
    network: NetworkArgs,

    /// The path to read engine API messages from.
    #[arg(long = "engine-api-store", value_name = "PATH")]
    engine_api_store: PathBuf,

    /// The number of milliseconds between Engine API messages.
    #[arg(long = "interval", default_value_t = 1_000)]
    interval: u64,
}

impl Command {
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
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(self.network.addr, self.network.port)))
            .discovery_addr(SocketAddr::V4(SocketAddrV4::new(
                self.network.discovery.addr,
                self.network.discovery.port,
            )))
            .build(ProviderFactory::new(db, self.chain.clone()))
            .start_network()
            .await?;
        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        Ok(network)
    }

    /// Execute `debug replay-engine` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        let config = Config::default();

        // Add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        fs::create_dir_all(&db_path)?;

        // Initialize the database
        let db =
            Arc::new(init_db(db_path, DatabaseArguments::default().log_level(self.db.log_level))?);
        let provider_factory = ProviderFactory::new(db.clone(), self.chain.clone());

        let consensus: Arc<dyn Consensus> = Arc::new(BeaconConsensus::new(Arc::clone(&self.chain)));

        #[cfg(not(feature = "optimism"))]
        let evm_config = EthEvmConfig::default();

        #[cfg(feature = "optimism")]
        let evm_config = OptimismEvmConfig::default();

        // Configure blockchain tree
        let tree_externals = TreeExternals::new(
            provider_factory.clone(),
            Arc::clone(&consensus),
            EvmProcessorFactory::new(self.chain.clone(), evm_config),
        );
        let tree = BlockchainTree::new(tree_externals, BlockchainTreeConfig::default(), None)?;
        let blockchain_tree = ShareableBlockchainTree::new(tree);

        // Set up the blockchain provider
        let blockchain_db =
            BlockchainProvider::new(provider_factory.clone(), blockchain_tree.clone())?;

        // Set up network
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

        // Set up payload builder
        #[cfg(not(feature = "optimism"))]
        let payload_builder = reth_ethereum_payload_builder::EthereumPayloadBuilder::default();

        // Optimism's payload builder is implemented on the OptimismPayloadBuilder type.
        #[cfg(feature = "optimism")]
        let payload_builder = reth_optimism_payload_builder::OptimismPayloadBuilder::default();

        let payload_generator = BasicPayloadJobGenerator::with_builder(
            blockchain_db.clone(),
            NoopTransactionPool::default(),
            ctx.task_executor.clone(),
            BasicPayloadJobGeneratorConfig::default(),
            self.chain.clone(),
            payload_builder,
        );

        #[cfg(feature = "optimism")]
        let (payload_service, payload_builder): (
            _,
            PayloadBuilderHandle<OptimismEngineTypes>,
        ) = PayloadBuilderService::new(payload_generator, blockchain_db.canonical_state_stream());

        #[cfg(not(feature = "optimism"))]
        let (payload_service, payload_builder): (_, PayloadBuilderHandle<EthEngineTypes>) =
            PayloadBuilderService::new(payload_generator, blockchain_db.canonical_state_stream());

        ctx.task_executor.spawn_critical("payload builder service", payload_service);

        // Configure the consensus engine
        let network_client = network.fetch_client().await?;
        let (consensus_engine_tx, consensus_engine_rx) = mpsc::unbounded_channel();
        let (beacon_consensus_engine, beacon_engine_handle) = BeaconConsensusEngine::with_channel(
            network_client,
            Pipeline::builder().build(provider_factory),
            blockchain_db.clone(),
            Box::new(ctx.task_executor.clone()),
            Box::new(network),
            None,
            false,
            payload_builder,
            None,
            u64::MAX,
            consensus_engine_tx,
            consensus_engine_rx,
            EngineHooks::new(),
        )?;
        info!(target: "reth::cli", "Consensus engine initialized");

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        ctx.task_executor.spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        let engine_api_store = EngineApiStore::new(self.engine_api_store.clone());
        for filepath in engine_api_store.engine_messages_iter()? {
            let contents =
                fs::read(&filepath).wrap_err(format!("failed to read: {}", filepath.display()))?;
            let message = serde_json::from_slice(&contents)
                .wrap_err(format!("failed to parse: {}", filepath.display()))?;
            debug!(target: "reth::cli", filepath = %filepath.display(), ?message, "Forwarding Engine API message");
            match message {
                StoredEngineApiMessage::ForkchoiceUpdated { state, payload_attrs } => {
                    let response =
                        beacon_engine_handle.fork_choice_updated(state, payload_attrs).await?;
                    debug!(target: "reth::cli", ?response, "Received for forkchoice updated");
                }
                StoredEngineApiMessage::NewPayload { payload, cancun_fields } => {
                    let response = beacon_engine_handle.new_payload(payload, cancun_fields).await?;
                    debug!(target: "reth::cli", ?response, "Received for new payload");
                }
            };

            // Pause before next message
            tokio::time::sleep(Duration::from_millis(self.interval)).await;
        }

        info!(target: "reth::cli", "Finished replaying engine API messages");

        match rx.await? {
            Ok(()) => info!("Beacon consensus engine exited successfully"),
            Err(error) => {
                error!(target: "reth::cli", %error, "Beacon consensus engine exited with an error")
            }
        };

        Ok(())
    }
}
