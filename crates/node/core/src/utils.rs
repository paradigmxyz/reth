//! Utility functions for node startup and shutdown, for example path parsing and retrieving single
//! blocks from the network.

use alloy_eips::BlockHashOrNumber;
use alloy_primitives::hex;
use alloy_rpc_types_engine::{JwtError, JwtSecret};
use eyre::Result;
use reth_consensus::Consensus;
use reth_network_p2p::{
    bodies::client::BodiesClient, headers::client::HeadersClient, priority::Priority,
};
use reth_primitives_traits::{Block, SealedBlock, SealedHeader};
use std::{
    io,
    path::{Path, PathBuf},
};
use tracing::{debug, info};

/// Parses a user-specified path into a [`PathBuf`].
pub fn parse_path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

/// Attempts to retrieve or create a JWT secret from the specified path.
///
/// A newly created secret file is owner-only on Unix.
pub fn get_or_create_jwt_secret_from_path(path: &Path) -> Result<JwtSecret, JwtError> {
    if path.exists() {
        debug!(target: "reth::cli", ?path, "Reading JWT auth secret file");
        return JwtSecret::from_file(path)
    }

    info!(target: "reth::cli", ?path, "Creating JWT auth secret file");
    if let Some(dir) = path.parent() {
        reth_fs_util::create_dir_all(dir).map_err(|err| JwtError::CreateDir {
            source: io::Error::other(err),
            path: dir.into(),
        })?;
    }

    let secret = JwtSecret::random();
    reth_fs_util::write_private(path, hex::encode(secret.as_bytes()))
        .map_err(|err| JwtError::Write { source: io::Error::other(err), path: path.into() })?;
    Ok(secret)
}

/// Get a single header from the network
pub async fn get_single_header<Client>(
    client: Client,
    id: BlockHashOrNumber,
) -> Result<SealedHeader<Client::Header>>
where
    Client: HeadersClient<Header: reth_primitives_traits::BlockHeader>,
{
    let (peer_id, response) = client.get_header_with_priority(id, Priority::High).await?.split();

    let Some(header) = response else {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of headers received. Expected: 1. Received: 0");
    };

    let header = SealedHeader::seal_slow(header);

    if !header.num_hash().matches_block_or_num(&id) {
        client.report_bad_message(peer_id);
        eyre::bail!(
            "Received invalid header. Received: {:?}. Expected: {:?}",
            header.num_hash(),
            id
        );
    }

    Ok(header)
}

/// Get a body from the network based on header
pub async fn get_single_body<B, Client>(
    client: Client,
    header: SealedHeader<B::Header>,
    consensus: impl Consensus<B>,
) -> Result<SealedBlock<B>>
where
    B: Block,
    Client: BodiesClient<Body = B::Body>,
{
    let (peer_id, response) = client.get_block_body(header.hash()).await?.split();

    let Some(body) = response else {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of bodies received. Expected: 1. Received: 0");
    };

    let block = SealedBlock::from_sealed_parts(header, body);
    consensus.validate_block_pre_execution(&block)?;

    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_or_create_jwt_secret_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jwt.hex");

        let secret = get_or_create_jwt_secret_from_path(&path).unwrap();

        assert_eq!(JwtSecret::from_file(&path).unwrap(), secret);
    }

    #[cfg(unix)]
    #[test]
    fn get_or_create_jwt_secret_creates_owner_only_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jwt.hex");

        get_or_create_jwt_secret_from_path(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
