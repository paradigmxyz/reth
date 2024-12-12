use reth_cli::chainspec::{parse_genesis, ChainSpecParser};
use reth_scroll_chainspec::{ScrollChainSpec, SCROLL_DEV, SCROLL_MAINNET, SCROLL_SEPOLIA};
use std::sync::Arc;

/// The parser for the Scroll chain specification.
#[derive(Debug, Clone)]
pub struct ScrollChainSpecParser;

impl ChainSpecParser for ScrollChainSpecParser {
    type ChainSpec = ScrollChainSpec;
    const SUPPORTED_CHAINS: &'static [&'static str] = &["dev", "scroll-mainnet", "scroll-sepolia"];

    fn parse(s: &str) -> eyre::Result<Arc<Self::ChainSpec>> {
        Ok(match s {
            "dev" => SCROLL_DEV.clone(),
            "scroll-mainnet" => SCROLL_MAINNET.clone(),
            "scroll-sepolia" => SCROLL_SEPOLIA.clone(),
            _ => Arc::new(parse_genesis(s)?.into()),
        })
    }
}
