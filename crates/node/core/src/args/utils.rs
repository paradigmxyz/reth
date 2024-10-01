//! Clap parser utilities

use std::{path::PathBuf, sync::Arc};

use alloy_genesis::Genesis;
use reth_chainspec::ChainSpec;
#[cfg(not(feature = "optimism"))]
use reth_chainspec::{DEV, HOLESKY, MAINNET, SEPOLIA};
use reth_cli::chainspec::ChainSpecParser;
use reth_fs_util as fs;
#[cfg(feature = "optimism")]
use reth_optimism_chainspec::{BASE_MAINNET, BASE_SEPOLIA, OP_DEV, OP_MAINNET, OP_SEPOLIA};

#[cfg(feature = "optimism")]
/// Chains supported by op-reth. First value should be used as the default.
pub const SUPPORTED_CHAINS: &[&str] =
    &["optimism", "optimism-sepolia", "base", "base-sepolia", "dev"];
#[cfg(not(feature = "optimism"))]
/// Chains supported by reth. First value should be used as the default.
pub const SUPPORTED_CHAINS: &[&str] = &["mainnet", "sepolia", "holesky", "dev"];

/// Clap value parser for [`ChainSpec`]s.
///
/// The value parser matches either a known chain, the path
/// to a json file, or a json formatted string in-memory. The json needs to be a Genesis struct.
#[cfg(not(feature = "optimism"))]
pub fn chain_value_parser(s: &str) -> eyre::Result<Arc<ChainSpec>, eyre::Error> {
    Ok(match s {
        "mainnet" => MAINNET.clone(),
        "sepolia" => SEPOLIA.clone(),
        "holesky" => HOLESKY.clone(),
        "dev" => DEV.clone(),
        _ => Arc::new(parse_custom_chain_spec(s)?),
    })
}

/// Clap value parser for [`OpChainSpec`](reth_optimism_chainspec::OpChainSpec)s.
///
/// The value parser matches either a known chain, the path
/// to a json file, or a json formatted string in-memory. The json needs to be a Genesis struct.
#[cfg(feature = "optimism")]
pub fn chain_value_parser(s: &str) -> eyre::Result<Arc<ChainSpec>, eyre::Error> {
    Ok(Arc::new(match s {
        "optimism" => OP_MAINNET.inner.clone(),
        "optimism_sepolia" | "optimism-sepolia" => OP_SEPOLIA.inner.clone(),
        "base" => BASE_MAINNET.inner.clone(),
        "base_sepolia" | "base-sepolia" => BASE_SEPOLIA.inner.clone(),
        "dev" => OP_DEV.inner.clone(),
        _ => parse_custom_chain_spec(s)?,
    }))
}

/// Parses a custom [`ChainSpec`].
pub fn parse_custom_chain_spec(s: &str) -> eyre::Result<ChainSpec, eyre::Error> {
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

    Ok(genesis.into())
}

/// A chain specification parser for ethereum chains.
#[derive(Debug, Copy, Clone, Default)]
#[non_exhaustive]
pub struct EthereumChainSpecParser;

impl ChainSpecParser for EthereumChainSpecParser {
    type ChainSpec = ChainSpec;

    const SUPPORTED_CHAINS: &'static [&'static str] = SUPPORTED_CHAINS;

    fn parse(s: &str) -> eyre::Result<Arc<ChainSpec>> {
        chain_value_parser(s)
    }
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
