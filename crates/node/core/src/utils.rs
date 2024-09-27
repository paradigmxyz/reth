//! Utility functions for node startup and shutdown, for example path parsing and retrieving single
//! blocks from the network.

use alloy_primitives::Sealable;
use alloy_rpc_types_engine::{JwtError, JwtSecret};
use eyre::Result;
use reth_chainspec::ChainSpec;
use reth_consensus_common::validation::validate_block_pre_execution;
use reth_network_p2p::{
    bodies::client::BodiesClient,
    headers::client::{HeadersClient, HeadersDirection, HeadersRequest},
    priority::Priority,
};
use reth_primitives::{BlockHashOrNumber, SealedBlock, SealedHeader};
use std::{
    env::VarError,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{debug, info};

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
        JwtSecret::try_create_random(path)
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

    let sealed_header = response.into_iter().next().unwrap().seal_slow();
    let (header, seal) = sealed_header.into_parts();
    let header = SealedHeader::new(header, seal);

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

    let body = response.unwrap();
    let block = SealedBlock { header, body };

    validate_block_pre_execution(&block, &chain_spec)?;

    Ok(block)
}
