//! Utility functions for node startup and shutdown, for example path parsing and retrieving single
//! blocks from the network.

use alloy_eips::BlockHashOrNumber;
use alloy_rpc_types_engine::{JwtError, JwtSecret};
use eyre::Result;
use reth_consensus::Consensus;
use reth_network_p2p::{
    bodies::client::BodiesClient, headers::client::HeadersClient, priority::Priority,
};
use reth_primitives_traits::{Block, SealedBlock, SealedHeader};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Parses a user-specified path into a [`PathBuf`].
pub fn parse_path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

/// Attempts to retrieve or create a JWT secret from the specified path.
///
/// On Unix, a newly created secret file is only accessible by the current user (`0600`). An
/// existing file is never modified, because it may be shared with the consensus client on purpose,
/// but a warning is logged if it is accessible by other users.
pub fn get_or_create_jwt_secret_from_path(path: &Path) -> Result<JwtSecret, JwtError> {
    if path.exists() {
        debug!(target: "reth::cli", ?path, "Reading JWT auth secret file");
        #[cfg(unix)]
        warn_if_jwt_secret_file_is_shared(path);
        JwtSecret::from_file(path)
    } else {
        info!(target: "reth::cli", ?path, "Creating JWT auth secret file");
        create_jwt_secret_file(path)
    }
}

/// Creates a random [`JwtSecret`] and stores it at `path` with `0600` permissions.
///
/// The file is created with the restrictive mode from the start, so the secret is never readable
/// by other users, unlike [`JwtSecret::try_create_random`] which relies on the process umask.
#[cfg(unix)]
fn create_jwt_secret_file(path: &Path) -> Result<JwtSecret, JwtError> {
    use alloy_primitives::hex;
    use std::{fs, io::Write, os::unix::fs::OpenOptionsExt};

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|source| JwtError::CreateDir { source, path: dir.into() })?;
    }

    let secret = JwtSecret::random();
    // `create_new` fails if the path already exists, including dangling symlinks, so a
    // pre-planted file or link is never written through.
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(hex::encode(secret.as_bytes()).as_bytes()))
        .map_err(|source| JwtError::Write { source, path: path.into() })?;
    Ok(secret)
}

#[cfg(not(unix))]
fn create_jwt_secret_file(path: &Path) -> Result<JwtSecret, JwtError> {
    JwtSecret::try_create_random(path)
}

/// Logs a warning if the JWT secret file at `path` is accessible by group or other users.
#[cfg(unix)]
fn warn_if_jwt_secret_file_is_shared(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    use tracing::warn;

    if let Ok(metadata) = std::fs::metadata(path) {
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            warn!(
                target: "reth::cli",
                ?path,
                mode = format_args!("{mode:03o}"),
                "JWT auth secret file is accessible by other users, consider restricting it to the owner (chmod 600)"
            );
        }
    }
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use alloy_primitives::hex;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn jwt_secret_file_is_created_with_owner_only_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("jwt.hex");

        let secret = get_or_create_jwt_secret_from_path(&path).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

        let read_back = get_or_create_jwt_secret_from_path(&path).unwrap();
        assert_eq!(secret.as_bytes(), read_back.as_bytes());
    }

    #[test]
    fn existing_jwt_secret_file_permissions_are_not_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jwt.hex");
        fs::write(&path, hex::encode(JwtSecret::random().as_bytes())).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        get_or_create_jwt_secret_from_path(&path).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o644);
    }
}
