//! Utility functions for node startup and shutdown, for example path parsing and retrieving single
//! blocks from the network.

use eyre::Result;
use reth_consensus_common::validation::validate_block_standalone;
use reth_interfaces::p2p::{
    bodies::client::BodiesClient,
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
};
use reth_network::NetworkManager;
use reth_primitives::{
    fs, BlockHashOrNumber, ChainSpec, HeadersDirection, SealedBlock, SealedHeader,
};
use reth_provider::BlockReader;
use reth_rpc::{JwtError, JwtSecret};
use std::{
    env::VarError,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{debug, info, trace, warn};

/// Parses a user-specified path with support for environment variables and common shorthands (e.g.
/// ~ for the user's home directory).
pub fn parse_path(value: &str) -> Result<PathBuf, shellexpand::LookupError<VarError>> {
    shellexpand::full(value).map(|path| PathBuf::from(path.into_owned()))
}

/// Attempts to retrieve or create a JWT secret from the specified path.
pub fn get_or_create_jwt_secret_from_path(path: &Path) -> Result<JwtSecret, JwtError> {
    if path.exists() {
        debug!(target: "reth::cli", ?path, "Reading JWT auth secret file");
        JwtSecret::from_file(path)
    } else {
        info!(target: "reth::cli", ?path, "Creating JWT auth secret file");
        JwtSecret::try_create(path)
    }
}

/// Collect the peers from the [NetworkManager] and write them to the given `persistent_peers_file`,
/// if configured.
pub fn write_peers_to_file<C>(network: &NetworkManager<C>, persistent_peers_file: Option<PathBuf>)
where
    C: BlockReader + Unpin,
{
    if let Some(file_path) = persistent_peers_file {
        let known_peers = network.all_peers().collect::<Vec<_>>();
        if let Ok(known_peers) = serde_json::to_string_pretty(&known_peers) {
            trace!(target: "reth::cli", peers_file =?file_path, num_peers=%known_peers.len(), "Saving current peers");
            let parent_dir = file_path.parent().map(fs::create_dir_all).transpose();
            match parent_dir.and_then(|_| fs::write(&file_path, known_peers)) {
                Ok(_) => {
                    info!(target: "reth::cli", peers_file=?file_path, "Wrote network peers to file");
                }
                Err(err) => {
                    warn!(target: "reth::cli", %err, peers_file=?file_path, "Failed to write network peers to file");
                }
            }
        }
    }
}

/// Get a single header from network
pub async fn get_single_header<Client>(
    client: Client,
    id: BlockHashOrNumber,
) -> Result<SealedHeader>
where
    Client: HeadersClient,
{
    let request = HeadersRequest { direction: HeadersDirection::Rising, limit: 1, start: id };

    let (peer_id, response) =
        client.get_headers_with_priority(request, Priority::High).await?.split();

    if response.len() != 1 {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of headers received. Expected: 1. Received: {}", response.len())
    }

    let header = response.into_iter().next().unwrap().seal_slow();

    let valid = match id {
        BlockHashOrNumber::Hash(hash) => header.hash() == hash,
        BlockHashOrNumber::Number(number) => header.number == number,
    };

    if !valid {
        client.report_bad_message(peer_id);
        eyre::bail!(
            "Received invalid header. Received: {:?}. Expected: {:?}",
            header.num_hash(),
            id
        );
    }

    Ok(header)
}

/// Get a body from network based on header
pub async fn get_single_body<Client>(
    client: Client,
    chain_spec: Arc<ChainSpec>,
    header: SealedHeader,
) -> Result<SealedBlock>
where
    Client: BodiesClient,
{
    let (peer_id, response) = client.get_block_body(header.hash()).await?.split();

    if response.is_none() {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of bodies received. Expected: 1. Received: 0")
    }

    let block = response.unwrap();
    let block = SealedBlock {
        header,
        body: block.transactions,
        ommers: block.ommers,
        withdrawals: block.withdrawals,
    };

    validate_block_standalone(&block, &chain_spec)?;

    Ok(block)
}
