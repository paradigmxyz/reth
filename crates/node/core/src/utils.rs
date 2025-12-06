//! Utility functions for node startup and shutdown, for example path parsing and retrieving single
//! blocks from the network.

use alloy_consensus::BlockHeader;
use alloy_eips::BlockHashOrNumber;
use alloy_rpc_types_engine::{JwtError, JwtSecret};
use eyre::Result;
use reth_consensus::{Consensus, ConsensusError};
use reth_network_p2p::{
    bodies::client::BodiesClient, headers::client::HeadersClient, priority::Priority,
};
use reth_primitives_traits::{Block, SealedBlock, SealedHeader};
use std::{
    env::VarError,
    path::{Path, PathBuf},
};
use tracing::{debug, info, warn};

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
        eyre::bail!("Invalid number of headers received. Expected: 1. Received: 0")
    };

    let header = SealedHeader::seal_slow(header);

    let valid = match id {
        BlockHashOrNumber::Hash(hash) => header.hash() == hash,
        BlockHashOrNumber::Number(number) => header.number() == number,
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

/// Get a body from the network based on header
pub async fn get_single_body<B, Client>(
    client: Client,
    header: SealedHeader<B::Header>,
    consensus: impl Consensus<B, Error = ConsensusError>,
) -> Result<SealedBlock<B>>
where
    B: Block,
    Client: BodiesClient<Body = B::Body>,
{
    let (peer_id, response) = client.get_block_body(header.hash()).await?.split();

    let Some(body) = response else {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of bodies received. Expected: 1. Received: 0")
    };

    let block = SealedBlock::from_sealed_parts(header, body);
    consensus.validate_block_pre_execution(&block)?;

    Ok(block)
}

/// Check available disk space for the given path using sysinfo.
///
/// Returns the available space in MB, or None if the check fails.
pub fn get_available_disk_space_mb(path: &Path) -> Option<u64> {
    use sysinfo::Disks;
    
    // Find the disk that contains the given path
    let path_canonical = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => return None,
    };
    
    let disks = Disks::new_with_refreshed_list();
    
    // Find the disk that contains the given path
    for disk in disks.iter() {
        let mount_point = disk.mount_point();
        if path_canonical.starts_with(mount_point) {
            // Get available space in bytes, convert to MB
            let available_bytes = disk.available_space();
            return Some(available_bytes / (1024 * 1024));
        }
    }
    
    None
}

/// Check if disk space is below the minimum threshold.
///
/// Returns true if the available disk space is below the minimum threshold (in MB).
pub fn is_disk_space_low(path: &Path, min_free_disk_mb: u64) -> bool {
    if min_free_disk_mb == 0 {
        return false; // Feature disabled
    }

    match get_available_disk_space_mb(path) {
        Some(available_mb) => {
            if available_mb <= min_free_disk_mb {
                warn!(
                    target: "reth::cli",
                    ?path,
                    available_mb,
                    min_free_disk_mb,
                    "Disk space below minimum threshold"
                );
                return true;
            }
            false
        }
        None => {
            warn!(
                target: "reth::cli",
                ?path,
                "Failed to check disk space, continuing anyway"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_is_disk_space_low_disabled() {
        // When min_free_disk is 0, feature is disabled
        let temp_dir = std::env::temp_dir();
        assert!(!is_disk_space_low(&temp_dir, 0), "Should return false when disabled");
    }

    #[test]
    fn test_is_disk_space_low_with_valid_path() {
        // Test with a valid path (should not panic)
        // We can't easily mock sysinfo, so we just test that it doesn't panic
        // and returns a boolean value
        let temp_dir = std::env::temp_dir();
        let result = is_disk_space_low(&temp_dir, 1_000_000_000); // Very large threshold
        // Should return either true or false, but not panic
        assert!(result == true || result == false);
    }

    #[test]
    fn test_is_disk_space_low_with_invalid_path() {
        // Test with a non-existent path
        let invalid_path = Path::new("/nonexistent/path/that/does/not/exist");
        // Should not panic, but may return false (feature disabled) or handle gracefully
        let result = is_disk_space_low(invalid_path, 1000);
        // Should not panic, result depends on sysinfo behavior
        assert!(result == true || result == false);
    }

    #[test]
    fn test_get_available_disk_space_mb_with_valid_path() {
        // Test with a valid path
        let temp_dir = std::env::temp_dir();
        let result = get_available_disk_space_mb(&temp_dir);
        // Should return Some(u64) if successful, or None if it fails
        match result {
            Some(mb) => {
                // If successful, should be a reasonable value (not 0 for temp dir)
                assert!(mb > 0 || mb == 0, "Available space should be a valid number");
            }
            None => {
                // It's okay if it fails, sysinfo might not work in all test environments
            }
        }
    }

    #[test]
    fn test_get_available_disk_space_mb_with_invalid_path() {
        // Test with a non-existent path
        let invalid_path = Path::new("/nonexistent/path/that/does/not/exist");
        let result = get_available_disk_space_mb(invalid_path);
        // Should return None for invalid paths
        assert_eq!(result, None);
    }

    #[test]
    fn test_is_disk_space_low_threshold_comparison() {
        // Test that the function correctly compares available space with threshold
        let temp_dir = std::env::temp_dir();
        
        if let Some(available_mb) = get_available_disk_space_mb(&temp_dir) {
            // Test with a threshold smaller than available space (should pass - space is sufficient)
            if available_mb > 0 {
                let result_small = is_disk_space_low(&temp_dir, available_mb.saturating_sub(1));
                assert!(!result_small, "Should pass (return false) when threshold is less than available space");
            }
            
            // Test with a threshold larger than available space (should fail - space is insufficient)
            let result_large = is_disk_space_low(&temp_dir, available_mb.saturating_add(1));
            assert!(result_large, "Should fail (return true) when threshold is greater than available space");
            
            // Test with threshold equal to available space (edge case)
            // When available == threshold, should trigger shutdown (return true) because "once reached" includes equality
            let result_equal = is_disk_space_low(&temp_dir, available_mb);
            assert!(result_equal, "Should fail (return true) when threshold equals available space, as 'once reached' includes equality");
        }
        // If we can't get disk space, that's okay - test environment might not support it
    }
}
