//! Main node command
//!
//! Starts the client
use clap::{crate_version, Parser};
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
use reth_primitives::{hex_literal::hex, Bytes, Header, H256, U256};
use reth_provider::{db_provider::ProviderImpl, BlockProvider, HeaderProvider};
use reth_stages::stages::{bodies::BodyStage, headers::HeaderStage, senders::SendersStage};
use std::{path::Path, str::FromStr, sync::Arc};
use tracing::{debug, info};

/// Start the client
#[derive(Debug, Parser)]
pub struct Command {
    /// Path to database folder
    // TODO: This should use dirs-next
    #[arg(long, default_value = "~/.reth/db")]
    db: String,
}

impl Command {
    /// Execute `node` command
    // TODO: RPC, metrics
    pub async fn execute(&self) -> eyre::Result<()> {
        info!("reth {} starting", crate_version!());

        let path = shellexpand::full(&self.db)?.into_owned();
        let expanded_db_path = Path::new(&path);
        let db = Arc::new(init_db(expanded_db_path)?);
        info!("Database ready");

        // TODO: Write genesis info
        // TODO: Here we should parse and validate the chainspec and build consensus/networking
        // stuff off of that
        let consensus = Arc::new(EthConsensus::new(consensus_config()));
        init_genesis(db.clone())?;

        info!("Connecting to p2p");
        let network = start_network(network_config(db.clone())).await?;

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
            .push(SendersStage { batch_size: 1000, commit_threshold: 100 }, false);

        // Run pipeline
        info!("Starting pipeline");
        // TODO: This is a temporary measure to set the fork choice state, but this should be
        // handled by the engine API
        consensus.notify_fork_choice_state(ForkchoiceState {
            // NOTE: This is block 1000
            head_block_hash: H256(hex!(
                "5b4590a9905fa1c9cc273f32e6dc63b4c512f0ee14edc6fa41c26b416a7b5d58"
            )),
            safe_block_hash: H256(hex!(
                "5b4590a9905fa1c9cc273f32e6dc63b4c512f0ee14edc6fa41c26b416a7b5d58"
            )),
            finalized_block_hash: H256(hex!(
                "5b4590a9905fa1c9cc273f32e6dc63b4c512f0ee14edc6fa41c26b416a7b5d58"
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
fn init_genesis<DB: Database>(db: Arc<DB>) -> Result<(), reth_db::Error> {
    let tx = db.tx_mut()?;
    let has_block = tx.cursor::<tables::CanonicalHeaders>()?.first()?.is_some();
    if has_block {
        debug!("Genesis already written, skipping.");
        return Ok(())
    }

    debug!("Writing genesis block.");

    // TODO: Should be based on a chain spec
    let mut genesis = Header::default();
    genesis.gas_limit = 5000;
    genesis.difficulty = U256::from(0x400000000usize);
    genesis.nonce = 0x0000000000000042;
    genesis.extra_data =
        Bytes::from_str("11bbe8db4e347b4e8c937c1c8370e4b5ed33adb3db69cbdb7a38e1e50b1b82fa")
            .unwrap()
            .0;
    genesis.state_root =
        H256(hex!("d7f8974fb5ac78d9ac099b9ad5018bedc2ce0a72dad1827a1709da30580f0544"));
    let genesis = genesis.seal();

    // Insert genesis
    tx.put::<tables::CanonicalHeaders>(genesis.number, genesis.hash())?;
    tx.put::<tables::Headers>(genesis.num_hash().into(), genesis.as_ref().clone())?;
    tx.put::<tables::HeaderNumbers>(genesis.hash(), genesis.number)?;
    tx.put::<tables::CumulativeTxCount>(genesis.num_hash().into(), 0)?;
    tx.put::<tables::HeaderTD>(genesis.num_hash().into(), genesis.difficulty.into())?;
    tx.commit()?;

    // TODO: Account balances
    Ok(())
}

// TODO: This should be based on some external config
fn network_config<DB: Database>(db: Arc<DB>) -> NetworkConfig<ProviderImpl<DB>> {
    NetworkConfig::builder(Arc::new(ProviderImpl::new(db)), rng_secret_key())
        .boot_nodes(mainnet_nodes())
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
    //tokio::task::spawn(txpool);
    tokio::task::spawn(eth);
    Ok(handle)
}
