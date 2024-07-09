//! Clap parser utilities

use alloy_genesis::Genesis;
use reth_chainspec::ChainSpec;
use reth_fs_util as fs;
use std::{path::PathBuf, sync::Arc};

use reth_chainspec::DEV;

#[cfg(feature = "optimism")]
use reth_chainspec::{BASE_MAINNET, BASE_SEPOLIA, OP_MAINNET, OP_SEPOLIA};

#[cfg(not(feature = "optimism"))]
use reth_chainspec::{HOLESKY, MAINNET, SEPOLIA};

#[cfg(feature = "optimism")]
/// Chains supported by op-reth. First value should be used as the default.
pub const SUPPORTED_CHAINS: &[&str] = &["optimism", "optimism-sepolia", "base", "base-sepolia"];
#[cfg(not(feature = "optimism"))]
/// Chains supported by reth. First value should be used as the default.
pub const SUPPORTED_CHAINS: &[&str] = &["mainnet", "sepolia", "holesky", "dev"];

/// The help info for the --chain flag
pub fn chain_help() -> String {
    format!("The chain this node is running.\nPossible values are either a built-in chain or the path to a chain specification file.\n\nBuilt-in chains:\n    {}", SUPPORTED_CHAINS.join(", "))
}

/// Clap value parser for [`ChainSpec`]s.
///
/// The value parser matches either a known chain, the path
/// to a json file, or a json formatted string in-memory. The json needs to be a Genesis struct.
pub fn chain_value_parser(s: &str) -> eyre::Result<Arc<ChainSpec>, eyre::Error> {
    Ok(match s {
        #[cfg(not(feature = "optimism"))]
        "mainnet" => MAINNET.clone(),
        #[cfg(not(feature = "optimism"))]
        "sepolia" => SEPOLIA.clone(),
        #[cfg(not(feature = "optimism"))]
        "holesky" => HOLESKY.clone(),
        "dev" => DEV.clone(),
        #[cfg(feature = "optimism")]
        "optimism" => OP_MAINNET.clone(),
        #[cfg(feature = "optimism")]
        "optimism_sepolia" | "optimism-sepolia" => OP_SEPOLIA.clone(),
        #[cfg(feature = "optimism")]
        "base" => BASE_MAINNET.clone(),
        #[cfg(feature = "optimism")]
        "base_sepolia" | "base-sepolia" => BASE_SEPOLIA.clone(),
        _ => {
            // try to read json from path first
            let raw = match fs::read_to_string(PathBuf::from(shellexpand::full(s)?.into_owned())) {
                Ok(raw) => raw,
                Err(io_err) => {
                    // valid json may start with "\n", but must contain "{"
                    if s.contains('{') {
                        s.to_string()
                    } else {
                        return Err(io_err.into()) // assume invalid path
                    }
                }
            };

            // both serialized Genesis and ChainSpec structs supported
            let genesis: Genesis = serde_json::from_str(&raw)?;

            Arc::new(genesis.into())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_chain_spec() {
        for chain in SUPPORTED_CHAINS {
            chain_value_parser(chain).unwrap();
        }
    }
}
