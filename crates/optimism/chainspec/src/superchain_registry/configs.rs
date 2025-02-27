use crate::superchain_registry::chain_metadata::{to_genesis_chain_config, ChainMetadata};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use alloy_genesis::Genesis;
use miniz_oxide::inflate::decompress_to_vec_zlib_with_limit;
use tar_no_std::{CorruptDataError, TarArchiveRef};

#[derive(Debug, thiserror::Error)]
pub(crate) enum SuperchainConfigError {
    #[error("Error reading archive due to corrupt data: {0}")]
    CorruptDataError(CorruptDataError),
    #[error("Error converting bytes to UTF-8 String: {0}")]
    FromUtf8Error(#[from] alloc::string::FromUtf8Error),
    #[error("Error reading file: {0}")]
    Utf8Error(#[from] core::str::Utf8Error),
    #[error("Error deserializing JSON: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("File {0} not found in archive")]
    FileNotFound(String),
    #[error("Error decompressing file: {0}")]
    DecompressError(String),
}

/// A genesis file can be up to 10MiB. This is a reasonable limit for the genesis file size.
const MAX_SIZE: usize = 16 * 1024 * 1024; // 16MiB

/// The tar file contains the chain configs and genesis files for all networks and environments.
const SUPER_CHAIN_CONFIGS_TAR_BYTES: &[u8] = include_bytes!("../../res/superchain-configs.tar");

/// Reads the [`Genesis`] from the superchain config tar file for network and environment.
/// For example, `read_genesis_from_superchain_config("base", "mainnet")`.
pub(crate) fn read_superchain_genesis(
    network: &str,
    environment: &str,
) -> Result<Genesis, SuperchainConfigError> {
    // Read the genesis file from the tar.
    let archive = TarArchiveRef::new(SUPER_CHAIN_CONFIGS_TAR_BYTES)
        .map_err(SuperchainConfigError::CorruptDataError)?;
    let genesis_file =
        read_file(&archive, &format!("genesis/{}/{}.json.deflate", environment, network))?;
    let file = decompress_to_vec_zlib_with_limit(&genesis_file, MAX_SIZE)
        .map_err(|e| SuperchainConfigError::DecompressError(format!("{}", e)))?;

    // TODO: Workaround because Genesis does not allow optional config
    // Some genesis files missing the config and have "config": null.
    // This is a workaround to patch the genesis file to allow the parsing.
    // See: https://github.com/ethereum-optimism/superchain-registry/issues/901
    let mut config_content = String::from_utf8(file)?;
    config_content = config_content.replace("\"config\":null,", "");

    // Update config from network config.
    let mut genesis: Genesis = serde_json::from_str(&config_content)?;

    // TODO: Remove this as well.
    // ChainConfig defaults to chain_id=1. So we know that the config is missing.
    if genesis.config.chain_id == 1 {
        let chain_config = read_superchain_metadata(network, environment, &archive)?;
        genesis.config = to_genesis_chain_config(&chain_config);
    }

    Ok(genesis)
}

/// Reads the [`ChainMetadata`] from the superchain config tar file for network and environment.
/// For example, `read_superchain_config("base", "mainnet")`.
fn read_superchain_metadata(
    name: &str,
    network: &str,
    archive: &TarArchiveRef<'_>,
) -> Result<ChainMetadata, SuperchainConfigError> {
    let config_file = read_file(archive, &format!("configs/{}/{}.json", network, name))?;
    let config_content = String::from_utf8(config_file)?;
    let chain_config: ChainMetadata = serde_json::from_str(&config_content)?;
    Ok(chain_config)
}

/// Reads a file from the tar archive. The file path is relative to the root of the tar archive.
fn read_file(
    archive: &TarArchiveRef<'_>,
    file_path: &str,
) -> Result<Vec<u8>, SuperchainConfigError> {
    for entry in archive.entries() {
        if entry.filename().as_str()? == file_path {
            return Ok(entry.data().to_vec())
        }
    }
    Err(SuperchainConfigError::FileNotFound(file_path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_optimism_primitives::ADDRESS_L2_TO_L1_MESSAGE_PASSER;
    use tar_no_std::TarArchiveRef;

    #[test]
    fn test_read_superchain_genesis() {
        let genesis = read_superchain_genesis("unichain", "mainnet").unwrap();
        assert_eq!(genesis.config.chain_id, 130);
        assert_eq!(genesis.timestamp, 1730748359);
        assert!(genesis.alloc.contains_key(&ADDRESS_L2_TO_L1_MESSAGE_PASSER));
    }

    #[test]
    fn test_read_superchain_genesis_with_workaround() {
        let genesis = read_superchain_genesis("base", "mainnet").unwrap();
        assert_eq!(genesis.config.chain_id, 8453);
        assert_eq!(genesis.timestamp, 1686789347);
        assert!(genesis.alloc.contains_key(&ADDRESS_L2_TO_L1_MESSAGE_PASSER));
    }

    #[test]
    fn test_read_superchain_metadata() {
        let archive = TarArchiveRef::new(SUPER_CHAIN_CONFIGS_TAR_BYTES).unwrap();
        let chain_config = read_superchain_metadata("base", "mainnet", &archive).unwrap();
        assert_eq!(chain_config.name, "Base");
        assert_eq!(chain_config.chain_id, 8453);
    }

    #[test]
    fn test_read_all_genesis_files() {
        let archive = TarArchiveRef::new(SUPER_CHAIN_CONFIGS_TAR_BYTES).unwrap();
        // Check that all genesis files are read without errors.
        for entry in archive.entries() {
            let filename = entry
                .filename()
                .as_str()
                .unwrap()
                .split('/')
                .map(|s| s.to_string())
                .collect::<Vec<String>>();
            if filename.first().unwrap().ne(&"genesis") {
                continue
            }
            read_superchain_metadata(
                &filename.get(2).unwrap().replace(".json.deflate", ""),
                filename.get(1).unwrap(),
                &archive,
            )
            .unwrap();
        }
    }
}
