#![cfg_attr(not(feature = "std"), no_std)]

use anyhow::{Result};
use alloy_primitives::{hex, B256, U256};
use alloy_consensus::Header;
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
    state_root_hex: &str,
    gas_limit_hex: &str,
    extra_data_hex: &str,
) -> Result<ChainSpec> {
    let base_fee = U256::from_be_slice(&hex::decode(base_fee_hex.trim_start_matches("0x")).unwrap_or_default());
    let ts = U256::from_be_slice(&hex::decode(timestamp_hex.trim_start_matches("0x")).unwrap_or_default());
    let gas_limit = U256::from_be_slice(&hex::decode(gas_limit_hex.trim_start_matches("0x")).unwrap_or_default());
    let mut state_root = [0u8; 32];
    let sr = hex::decode(state_root_hex.trim_start_matches("0x")).unwrap_or_default();
    let len = sr.len().min(32);
    state_root[32 - len..].copy_from_slice(&sr[sr.len() - len..]);

    let extra = hex::decode(extra_data_hex.trim_start_matches("0x")).unwrap_or_default();

    let mut header = Header::default();
    header.parent_hash = B256::ZERO;
    header.ommers_hash = B256::from_slice(
        &hex::decode("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0cf4f7b9327e38a").unwrap(),
    );
    header.beneficiary = Default::default();
    header.state_root = B256::from(state_root);
    header.transactions_root = B256::from_slice(
        &hex::decode("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").unwrap(),
    );
    header.receipts_root = B256::from_slice(
        &hex::decode("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").unwrap(),
    );
    header.logs_bloom = Default::default();
    header.difficulty = U256::ZERO;
    header.number = 0;
    header.gas_limit = gas_limit.to::<u64>();
    header.gas_used = 0;
    header.timestamp = ts.to::<u64>();
    header.extra_data = extra.into();
    header.mix_hash = B256::ZERO;
    header.nonce = 0u64.into();
    header.base_fee_per_gas = Some(base_fee.to::<u64>());
    header.withdrawals_root = None;
    header.blob_gas_used = Some(0);
    header.excess_blob_gas = Some(0);

    let genesis = Genesis::from_header(header);
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
