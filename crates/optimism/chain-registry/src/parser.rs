use std::{fs, path::PathBuf, sync::Arc};
use eyre::Context;
use reth_cli::chainspec::{parse_genesis, ChainSpecParser};
use reth_optimism_chainspec::OpChainSpec;

use crate::SuperChainRegistryManager;

/// Parser for the Op Superchain
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct OpSuperchainSpecParser;

impl ChainSpecParser for OpSuperchainSpecParser {
    type ChainSpec = OpChainSpec;

    const SUPPORTED_CHAINS: &'static [&'static str] = &[""];

    fn parse(s: &str) -> eyre::Result<Arc<Self::ChainSpec>> {
        chain_value_parser(s)
    }
}

pub fn chain_value_parser(s: &str) -> eyre::Result<Arc<OpChainSpec>, eyre::Error> {
    let network_type = match s.contains("sepolia") {
        true => "sepolia",
        false => "mainnet",
    };

    if s.contains("_sepolia") {
        s.strip_suffix("_sepolia");
    } else if s.contains("-sepolia") {
        s.strip_suffix("-sepolia");
    }

    let registry_manager = SuperChainRegistryManager::new("./")?;
    let chainspec = registry_manager.get_genesis(network_type, s)?;
    
    Ok(Arc::new(chainspec))
}
