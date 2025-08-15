use reth_cli::chainspec::{parse_genesis, ChainSpecParser};
use std::sync::Arc;


use alloy_chains::Chain;


#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

use revm::primitives::hardfork::SpecId;

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

pub struct ArbitrumChainSpecParser;

impl ChainSpecParser for ArbitrumChainSpecParser {
    fn parse_chain_or_path(
        &self,
        chain: Option<String>,
        maybe_path: Option<String>,
    ) -> eyre::Result<Arc<reth_chainspec::ChainSpec>> {
        if let Some(path) = maybe_path {
            if !path.is_empty() {
                return parse_genesis(path);
            }
        }

        if let Some(name) = chain.as_deref() {
            match name {
                "arbitrum-sepolia" | "arb-sepolia" | "arbsepolia" | "421614" => {
                    return Ok(Arc::new(arbitrum_sepolia_spec()))
                }
                _ => {}
            }
        }

        eyre::bail!("Unknown Arbitrum chain alias. Try --chain arbitrum-sepolia or pass a genesis file via --chain or --chain.path")
    }
}

use reth_cli::chainspec::{parse_genesis, ChainSpecParser};
use std::sync::Arc;


use alloy_chains::Chain;


#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

use revm::primitives::hardfork::SpecId;

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
