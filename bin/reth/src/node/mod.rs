//! Main node command
//!
//! Starts the client
use crate::{
    config::Config,
    dirs::{ConfigPath, DbPath},
    util::chainspec::{chain_spec_value_parser, ChainSpecification, Genesis},
};
use clap::{crate_version, Parser};
use eyre::WrapErr;
use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_util::layers::{PrefixLayer, Stack};
use reth_consensus::EthConsensus;
use reth_db::{
    cursor::DbCursorRO,
    database::Database,
    mdbx::{Env, WriteMap},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_downloaders::{bodies, headers};
use reth_interfaces::consensus::ForkchoiceState;
use reth_network::{
    config::{mainnet_nodes, rng_secret_key},
    error::NetworkError,
    NetworkConfig, NetworkHandle, NetworkManager,
};
use reth_primitives::{Account, Header, H256};
use reth_provider::{db_provider::ProviderImpl, BlockProvider, HeaderProvider};
use reth_stages::stages::{bodies::BodyStage, headers::HeaderStage, senders::SendersStage};
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
}

impl Command {
    /// Execute `node` command
    // TODO: RPC
    pub async fn execute(&self) -> eyre::Result<()> {
        let config: Config = confy::load_path(&self.config)?;
        info!("reth {} starting", crate_version!());

        info!("Opening database at {}", &self.db);
        let db = Arc::new(init_db(&self.db)?);
        info!("Database open");

        if let Some(listen_addr) = self.metrics {
            info!("Starting metrics endpoint at {}", listen_addr);
            let (recorder, exporter) = PrometheusBuilder::new()
                .with_http_listener(listen_addr)
                .build()
                .wrap_err("Could not build Prometheus endpoint.")?;
            tokio::task::spawn(exporter);
            Stack::new(recorder)
                .push(PrefixLayer::new("reth"))
                .install()
                .wrap_err("Couldn't set metrics recorder.")?;
        }

        let chain_id = self.chain.consensus.chain_id;
        let consensus = Arc::new(EthConsensus::new(self.chain.consensus.clone()));
        let genesis_hash = init_genesis(db.clone(), self.chain.genesis.clone())?;

        info!("Connecting to p2p");
        let network = start_network(network_config(db.clone(), chain_id, genesis_hash)).await?;

        // TODO: Are most of these Arcs unnecessary? For example, fetch client is completely
        // cloneable on its own
        // TODO: Remove magic numbers
        let fetch_client = Arc::new(network.fetch_client().await?);
        let mut pipeline = reth_stages::Pipeline::new()
            .push(HeaderStage {
                downloader: headers::linear::LinearDownloadBuilder::default()
                    .batch_size(config.stages.headers.downloader_batch_size)
                    .retries(config.stages.headers.downloader_retries)
                    .build(consensus.clone(), fetch_client.clone()),
                consensus: consensus.clone(),
                client: fetch_client.clone(),
                network_handle: network.clone(),
                commit_threshold: config.stages.headers.commit_threshold,
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
            .push(SendersStage {
                batch_size: config.stages.senders.batch_size,
                commit_threshold: config.stages.senders.commit_threshold,
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

// TODO: This should be based on some external config
fn network_config<DB: Database>(
    db: Arc<DB>,
    chain_id: u64,
    genesis_hash: H256,
) -> NetworkConfig<ProviderImpl<DB>> {
    NetworkConfig::builder(Arc::new(ProviderImpl::new(db)), rng_secret_key())
        .boot_nodes(mainnet_nodes())
        .genesis_hash(genesis_hash)
        .chain_id(chain_id)
        .build()
}

/// Starts the networking stack given a [NetworkConfig] and returns a handle to the network.
async fn start_network<C>(config: NetworkConfig<C>) -> Result<NetworkHandle, NetworkError>
where
    C: BlockProvider + HeaderProvider + 'static,
{
    let client = config.client.clone();
    let (handle, network, _txpool, eth) =
        NetworkManager::builder(config).await?.request_handler(client).split_with_handle();

    tokio::task::spawn(network);
    // TODO: tokio::task::spawn(txpool);
    tokio::task::spawn(eth);
    Ok(handle)
}
