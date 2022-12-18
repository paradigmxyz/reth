//! Main node command
//!
//! Starts the client
use crate::util::{chainspec::Genesis, parse_path};
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
use reth_primitives::{hex_literal::hex, Account, Header, H256};
use reth_provider::{db_provider::ProviderImpl, BlockProvider, HeaderProvider};
use reth_stages::stages::{bodies::BodyStage, headers::HeaderStage, senders::SendersStage};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{debug, info};

// TODO: Move this out somewhere
const MAINNET_GENESIS: &str = include_str!("../../res/chainspec/mainnet.json");

/// Start the client
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the database folder.
    // TODO: This should use dirs-next
    #[arg(long, value_name = "PATH", default_value = "~/.reth/db", value_parser = parse_path)]
    db: PathBuf,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[clap(long, value_name = "SOCKET")]
    metrics: Option<SocketAddr>,
}

impl Command {
    /// Execute `node` command
    // TODO: RPC, metrics
    pub async fn execute(&self) -> eyre::Result<()> {
        info!("reth {} starting", crate_version!());

        info!("Opening database at {}", &self.db.display());
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

        // TODO: More info from chainspec (chain ID etc.)
        let consensus = Arc::new(EthConsensus::new(consensus_config()));
        let genesis_hash =
            init_genesis(db.clone(), serde_json::from_str(MAINNET_GENESIS).unwrap())?;

        info!("Connecting to p2p");
        let network = start_network(network_config(db.clone(), genesis_hash)).await?;

        // TODO: Are most of these Arcs unnecessary? For example, fetch client is completely
        // cloneable on its own
        // TODO: Remove magic numbers
        let fetch_client = Arc::new(network.fetch_client().await?);
        let mut pipeline = reth_stages::Pipeline::new()
            .push(
                HeaderStage {
                    downloader: headers::linear::LinearDownloadBuilder::default()
                        .build(consensus.clone(), fetch_client.clone()),
                    consensus: consensus.clone(),
                    client: fetch_client.clone(),
                    network_handle: network.clone(),
                    commit_threshold: 100,
                },
                false,
            )
            .push(
                BodyStage {
                    downloader: Arc::new(bodies::concurrent::ConcurrentDownloader::new(
                        fetch_client.clone(),
                        consensus.clone(),
                    )),
                    consensus: consensus.clone(),
                    commit_threshold: 100,
                },
                false,
            )
            .push(SendersStage { batch_size: 100, commit_threshold: 1000 }, false);

        // Run pipeline
        info!("Starting pipeline");
        // TODO: This is a temporary measure to set the fork choice state, but this should be
        // handled by the engine API
        consensus.notify_fork_choice_state(ForkchoiceState {
            // NOTE: This is block 50,000. The first transaction ever is in block 46,147
            head_block_hash: H256(hex!(
                "0e30a7c0c1cee426011e274abc746c1ad3c48757433eb0139755658482498aa9"
            )),
            safe_block_hash: H256(hex!(
                "0e30a7c0c1cee426011e274abc746c1ad3c48757433eb0139755658482498aa9"
            )),
            finalized_block_hash: H256(hex!(
                "0e30a7c0c1cee426011e274abc746c1ad3c48757433eb0139755658482498aa9"
            )),
        })?;
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
    genesis_hash: H256,
) -> NetworkConfig<ProviderImpl<DB>> {
    NetworkConfig::builder(Arc::new(ProviderImpl::new(db)), rng_secret_key())
        .boot_nodes(mainnet_nodes())
        .genesis_hash(genesis_hash)
        .build()
}

// TODO: This should be based on some external config
fn consensus_config() -> reth_consensus::Config {
    reth_consensus::Config::default()
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
