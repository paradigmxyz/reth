//! Main node command
//!
//! Starts the client
use crate::{
    config::Config,
    dirs::{ConfigPath, DbPath},
    prometheus_exporter,
    util::chainspec::{chain_spec_value_parser, ChainSpecification, Genesis},
};
use clap::{crate_version, Parser};
use fdlimit::raise_fd_limit;
use reth_consensus::BeaconConsensus;
use reth_db::{
    cursor::DbCursorRO,
    database::Database,
    mdbx::{Env, WriteMap},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_downloaders::{bodies, headers};
use reth_executor::Config as ExecutorConfig;
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{Account, Header, H256};
use reth_stages::{
    metrics::HeaderMetrics,
    stages::{
        bodies::BodyStage, execution::ExecutionStage, headers::HeaderStage,
        sender_recovery::SenderRecoveryStage, total_difficulty::TotalDifficultyStage,
    },
};
use std::{net::SocketAddr, path::Path, sync::Arc};
use tracing::{debug, info};

/// Start the client
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(long, value_name = "PATH", verbatim_doc_comment, default_value_t)]
    db: DbPath,

    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment, default_value_t)]
    config: ConfigPath,

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
        value_parser = chain_spec_value_parser
    )]
    chain: ChainSpecification,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[clap(long, value_name = "SOCKET")]
    metrics: Option<SocketAddr>,

    /// Set the chain tip manually for testing purposes.
    ///
    /// NOTE: This is a temporary flag
    #[arg(long = "debug.tip")]
    tip: Option<H256>,

    /// Disable the discovery service.
    #[arg(short, long)]
    disable_discovery: bool,
}

impl Command {
    /// Execute `node` command
    // TODO: RPC
    pub async fn execute(&self) -> eyre::Result<()> {
        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit();

        let config: Config = confy::load_path(&self.config).unwrap_or_default();
        info!("reth {} starting", crate_version!());

        info!("Opening database at {}", &self.db);
        let db = Arc::new(init_db(&self.db)?);
        info!("Database open");

        if let Some(listen_addr) = self.metrics {
            info!("Starting metrics endpoint at {}", listen_addr);
            prometheus_exporter::initialize(listen_addr)?;
            HeaderMetrics::describe();
        }

        let chain_id = self.chain.consensus.chain_id;
        let consensus = Arc::new(BeaconConsensus::new(self.chain.consensus.clone()));
        let genesis_hash = init_genesis(db.clone(), self.chain.genesis.clone())?;

        let network = config
            .network_config(db.clone(), chain_id, genesis_hash, self.disable_discovery)
            .start_network()
            .await?;

        info!(peer_id = ?network.peer_id(), local_addr = %network.local_addr(), "Started p2p networking");

        // TODO: Are most of these Arcs unnecessary? For example, fetch client is completely
        // cloneable on its own
        // TODO: Remove magic numbers
        let fetch_client = Arc::new(network.fetch_client().await?);
        let mut pipeline = reth_stages::Pipeline::default()
            .with_sync_state_updater(network.clone())
            .push(HeaderStage {
                downloader: headers::linear::LinearDownloadBuilder::default()
                    .batch_size(config.stages.headers.downloader_batch_size)
                    .retries(config.stages.headers.downloader_retries)
                    .build(consensus.clone(), fetch_client.clone()),
                consensus: consensus.clone(),
                client: fetch_client.clone(),
                network_handle: network.clone(),
                commit_threshold: config.stages.headers.commit_threshold,
                metrics: HeaderMetrics::default(),
            })
            .push(TotalDifficultyStage {
                commit_threshold: config.stages.total_difficulty.commit_threshold,
            })
            .push(BodyStage {
                downloader: Arc::new(
                    bodies::concurrent::ConcurrentDownloader::new(
                        fetch_client.clone(),
                        consensus.clone(),
                    )
                    .with_batch_size(config.stages.bodies.downloader_batch_size)
                    .with_retries(config.stages.bodies.downloader_retries)
                    .with_concurrency(config.stages.bodies.downloader_concurrency),
                ),
                consensus: consensus.clone(),
                commit_threshold: config.stages.bodies.commit_threshold,
            })
            .push(SenderRecoveryStage {
                batch_size: config.stages.sender_recovery.batch_size,
                commit_threshold: config.stages.sender_recovery.commit_threshold,
            })
            .push(ExecutionStage {
                config: ExecutorConfig::new_ethereum(),
                commit_threshold: config.stages.execution.commit_threshold,
            });

        if let Some(tip) = self.tip {
            debug!("Tip manually set: {}", tip);
            consensus.notify_fork_choice_state(ForkchoiceState {
                head_block_hash: tip,
                safe_block_hash: tip,
                finalized_block_hash: tip,
            })?;
        }

        // Run pipeline
        info!("Starting pipeline");
        pipeline.run(db.clone()).await?;

        info!("Finishing up");
        Ok(())
    }
}

/// Opens up an existing database or creates a new one at the specified path.
fn init_db<P: AsRef<Path>>(path: P) -> eyre::Result<Env<WriteMap>> {
    std::fs::create_dir_all(path.as_ref())?;
    let db = reth_db::mdbx::Env::<reth_db::mdbx::WriteMap>::open(
        path.as_ref(),
        reth_db::mdbx::EnvKind::RW,
    )?;
    db.create_tables()?;

    Ok(db)
}

/// Write the genesis block if it has not already been written
#[allow(clippy::field_reassign_with_default)]
fn init_genesis<DB: Database>(db: Arc<DB>, genesis: Genesis) -> Result<H256, reth_db::Error> {
    let tx = db.tx_mut()?;
    if let Some((_, hash)) = tx.cursor::<tables::CanonicalHeaders>()?.first()? {
        debug!("Genesis already written, skipping.");
        return Ok(hash)
    }
    debug!("Writing genesis block.");

    // Insert account state
    for (address, account) in &genesis.alloc {
        tx.put::<tables::PlainAccountState>(
            *address,
            Account {
                nonce: account.nonce.unwrap_or_default(),
                balance: account.balance,
                bytecode_hash: None,
            },
        )?;
    }

    // Insert header
    let header: Header = genesis.into();
    let hash = header.hash_slow();
    tx.put::<tables::CanonicalHeaders>(0, hash)?;
    tx.put::<tables::HeaderNumbers>(hash, 0)?;
    tx.put::<tables::BlockBodies>((0, hash).into(), Default::default())?;
    tx.put::<tables::BlockTransitionIndex>((0, hash).into(), 0)?;
    tx.put::<tables::HeaderTD>((0, hash).into(), header.difficulty.into())?;
    tx.put::<tables::Headers>((0, hash).into(), header)?;

    tx.commit()?;
    Ok(hash)
}
