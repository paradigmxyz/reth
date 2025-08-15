#![cfg_attr(not(feature = "std"), no_std)]

use reth_cli::chainspec::{parse_genesis, ChainSpecParser};
use std::sync::Arc;

use alloy_chains::Chain;
use revm::primitives::hardfork::SpecId;

extern crate alloc;

pub trait ArbitrumChainSpec {
    fn chain_id(&self) -> u64;
    fn spec_id_by_timestamp(&self, _timestamp: u64) -> SpecId;
}

#[derive(Clone, Debug, Default)]
pub struct ArbChainSpec {
    pub chain_id: u64,
}

impl ArbitrumChainSpec for ArbChainSpec {
    fn chain_id(&self) -> u64 {
        self.chain_id
    }
    fn spec_id_by_timestamp(&self, _timestamp: u64) -> SpecId {
        SpecId::CANCUN
    }
}

impl ArbitrumChainSpec for reth_chainspec::ChainSpec {
    fn chain_id(&self) -> u64 {
        self.chain().id()
    }
    fn spec_id_by_timestamp(&self, _timestamp: u64) -> SpecId {
        SpecId::CANCUN
    }
}

pub fn arbitrum_sepolia_spec() -> reth_chainspec::ChainSpec {
    let mut spec = reth_chainspec::ChainSpec::default();
    spec.chain = Chain::from(421_614u64);
    spec
}

#[derive(Debug, Clone, Default)]
pub struct ArbitrumChainSpecParser;

impl ChainSpecParser for ArbitrumChainSpecParser {
    type ChainSpec = reth_chainspec::ChainSpec;

    const SUPPORTED_CHAINS: &'static [&'static str] = &[
        "arbitrum-sepolia",
        "arb-sepolia",
        "arbsepolia",
        "421614",
    ];

    fn parse(s: &str) -> eyre::Result<Arc<Self::ChainSpec>> {
        match s {
            "arbitrum-sepolia" | "arb-sepolia" | "arbsepolia" | "421614" => {
                Ok(Arc::new(arbitrum_sepolia_spec()))
            }
            _ => Ok(Arc::new(reth_chainspec::ChainSpec::from_genesis(parse_genesis(s)?))),
        }
    }
}
