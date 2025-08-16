#![cfg_attr(not(feature = "std"), no_std)]

use anyhow::Result;
use alloy_primitives::{hex, Address, B256, U256};
use alloy_genesis::Genesis;
use reth_chainspec::ChainSpec;

use reth_cli::chainspec::{parse_genesis, ChainSpecParser};
use std::sync::Arc;

use alloy_chains::Chain;
use revm::primitives::hardfork::SpecId;

extern crate alloc;

pub fn sepolia_baked_genesis_from_header(
    chain_id: u64,
    base_fee_hex: &str,
    timestamp_hex: &str,
    _state_root_hex: &str,
    gas_limit_hex: &str,
    extra_data_hex: &str,
) -> Result<ChainSpec> {
    let base_fee = U256::from_be_slice(&hex::decode(base_fee_hex.trim_start_matches("0x")).unwrap_or_default());
    let ts = U256::from_be_slice(&hex::decode(timestamp_hex.trim_start_matches("0x")).unwrap_or_default());
    let gas_limit = U256::from_be_slice(&hex::decode(gas_limit_hex.trim_start_matches("0x")).unwrap_or_default());
    let extra = hex::decode(extra_data_hex.trim_start_matches("0x")).unwrap_or_default();

    let mut genesis = Genesis::default();
    genesis.config.chain_id = chain_id;
    genesis.nonce = 0;
    genesis.timestamp = ts.to::<u64>();
    genesis.extra_data = extra.into();
    genesis.gas_limit = gas_limit.to::<u64>();
    genesis.difficulty = U256::ZERO;
    genesis.mix_hash = B256::ZERO;
    genesis.coinbase = Address::ZERO;
    genesis.base_fee_per_gas = Some(base_fee.to::<u128>());
    genesis.excess_blob_gas = Some(0);
    genesis.blob_gas_used = Some(0);
    genesis.number = Some(0);

    let spec = reth_chainspec::ChainSpec::builder()
        .chain(Chain::from(chain_id))
        .genesis(genesis)
        .build();
    Ok(spec)
}

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
