use super::{bsc::bsc_mainnet, BscChainSpec};
use reth_cli::chainspec::ChainSpecParser;
use std::sync::Arc;

/// Bsc chain specification parser.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct BscChainSpecParser;

impl ChainSpecParser for BscChainSpecParser {
    type ChainSpec = BscChainSpec;

    const SUPPORTED_CHAINS: &'static [&'static str] = &["bsc"];

    fn parse(s: &str) -> eyre::Result<Arc<Self::ChainSpec>> {
        chain_value_parser(s)
    }
}

/// Clap value parser for [`BscChainSpec`]s.
///
/// The value parser matches either a known chain, the path
/// to a json file, or a json formatted string in-memory. The json needs to be a Genesis struct.
pub fn chain_value_parser(s: &str) -> eyre::Result<Arc<BscChainSpec>> {
    match s {
        "bsc" => Ok(Arc::new(BscChainSpec { inner: bsc_mainnet() })),
        _ => Err(eyre::eyre!("Unsupported chain: {}", s)),
    }
}
