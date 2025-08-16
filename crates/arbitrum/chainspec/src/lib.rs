#![cfg_attr(not(feature = "std"), no_std)]

use anyhow::Result;
use alloy_primitives::{hex, Address, B256, U256, keccak256};
use alloy_genesis::{Genesis, GenesisAccount};
use reth_chainspec::ChainSpec;

use reth_cli::chainspec::{parse_genesis, ChainSpecParser};
use std::sync::Arc;

use alloy_chains::Chain;
use revm::primitives::hardfork::SpecId;

extern crate alloc;

use alloc::collections::BTreeMap;

const ARBOS_ADDR: Address = alloy_primitives::address!("0xA4B05FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF");

const VERSION_OFFSET: u64 = 0;
const UPGRADE_VERSION_OFFSET: u64 = 1;
const UPGRADE_TIMESTAMP_OFFSET: u64 = 2;
const NETWORK_FEE_ACCOUNT_OFFSET: u64 = 3;
const CHAIN_ID_OFFSET: u64 = 4;
const GENESIS_BLOCK_NUM_OFFSET: u64 = 5;
const INFRA_FEE_ACCOUNT_OFFSET: u64 = 6;
const BROTLI_LEVEL_OFFSET: u64 = 7;
const NATIVE_TOKEN_ENABLED_FROM_TIME_OFFSET: u64 = 8;

const L1_PRICING_SUBSPACE: u8 = 0;
const L2_PRICING_SUBSPACE: u8 = 1;

const L2_SPEED_LIMIT_PER_SECOND_OFFSET: u64 = 0;
const L2_PER_BLOCK_GAS_LIMIT_OFFSET: u64 = 1;
const L2_BASE_FEE_WEI_OFFSET: u64 = 2;
const L2_MIN_BASE_FEE_WEI_OFFSET: u64 = 3;
const L2_GAS_BACKLOG_OFFSET: u64 = 4;
const L2_PRICING_INERTIA_OFFSET: u64 = 5;
const L2_BACKLOG_TOLERANCE_OFFSET: u64 = 6;

const INITIAL_SPEED_LIMIT_PER_SECOND_V0: u64 = 1_000_000;
const INITIAL_PER_BLOCK_GAS_LIMIT_V0: u64 = 20 * 1_000_000;
const INITIAL_MIN_BASE_FEE_WEI: u128 = 100_000_000; // 0.1 gwei
const INITIAL_PRICING_INERTIA: u64 = 102;
const INITIAL_BACKLOG_TOLERANCE: u64 = 10;

fn be_u256(val: U256) -> B256 {
    B256::from(val.to_be_bytes::<32>())
}
fn be_u64(val: u64) -> B256 {
    be_u256(U256::from(val))
}
fn map_slot(storage_key: &[u8], key: B256) -> B256 {
    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(key.as_slice());
    let boundary = 31usize;
    let mut mapped = [0u8; 32];
    let hashed = keccak256([storage_key, &key_bytes[..boundary]].concat());
    mapped[..boundary].copy_from_slice(&hashed[..boundary]);
    mapped[boundary] = key_bytes[boundary];
    B256::from(mapped)
}
fn subspace(storage_key: &[u8], id: u8) -> Vec<u8> {
    keccak256([storage_key, &[id]].concat()).to_vec()
}

fn build_minimal_arbos_storage(chain_id: u64, initial_l1_base_fee: U256) -> BTreeMap<B256, B256> {
    let mut storage = BTreeMap::<B256, B256>::new();
    let root_key: Vec<u8> = Vec::new();

    storage.insert(map_slot(&root_key, be_u64(VERSION_OFFSET)), be_u64(1));
    storage.insert(map_slot(&root_key, be_u64(UPGRADE_VERSION_OFFSET)), be_u64(0));
    storage.insert(map_slot(&root_key, be_u64(UPGRADE_TIMESTAMP_OFFSET)), be_u64(0));
    storage.insert(map_slot(&root_key, be_u64(NETWORK_FEE_ACCOUNT_OFFSET)), B256::ZERO);
    storage.insert(map_slot(&root_key, be_u64(CHAIN_ID_OFFSET)), be_u256(U256::from(chain_id)));
    storage.insert(map_slot(&root_key, be_u64(GENESIS_BLOCK_NUM_OFFSET)), be_u64(0));
    storage.insert(map_slot(&root_key, be_u64(INFRA_FEE_ACCOUNT_OFFSET)), B256::ZERO);
    storage.insert(map_slot(&root_key, be_u64(BROTLI_LEVEL_OFFSET)), be_u64(0));
    storage.insert(map_slot(&root_key, be_u64(NATIVE_TOKEN_ENABLED_FROM_TIME_OFFSET)), be_u64(0));

    let l2_space = subspace(&root_key, L2_PRICING_SUBSPACE);
    storage.insert(map_slot(&l2_space, be_u64(L2_SPEED_LIMIT_PER_SECOND_OFFSET)), be_u64(INITIAL_SPEED_LIMIT_PER_SECOND_V0));
    storage.insert(map_slot(&l2_space, be_u64(L2_PER_BLOCK_GAS_LIMIT_OFFSET)), be_u64(INITIAL_PER_BLOCK_GAS_LIMIT_V0));
    storage.insert(map_slot(&l2_space, be_u64(L2_BASE_FEE_WEI_OFFSET)), be_u256(initial_l1_base_fee));
    storage.insert(map_slot(&l2_space, be_u64(L2_MIN_BASE_FEE_WEI_OFFSET)), be_u256(U256::from(INITIAL_MIN_BASE_FEE_WEI)));
    storage.insert(map_slot(&l2_space, be_u64(L2_GAS_BACKLOG_OFFSET)), be_u64(0));
    storage.insert(map_slot(&l2_space, be_u64(L2_PRICING_INERTIA_OFFSET)), be_u64(INITIAL_PRICING_INERTIA));
    storage.insert(map_slot(&l2_space, be_u64(L2_BACKLOG_TOLERANCE_OFFSET)), be_u64(INITIAL_BACKLOG_TOLERANCE));

    let l1_space = subspace(&root_key, L1_PRICING_SUBSPACE);
    let price_per_unit_offset: u64 = 7;
    storage.insert(map_slot(&l1_space, be_u64(price_per_unit_offset)), be_u256(initial_l1_base_fee));

    storage
}

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
    genesis.config.london_block = Some(0);
    genesis.config.cancun_time = Some(0);

    genesis.nonce = 0;
    genesis.timestamp = ts.to::<u64>();
    genesis.extra_data = extra.into();
    genesis.gas_limit = gas_limit.to::<u64>();
    genesis.difficulty = U256::ZERO;
    genesis.mix_hash = B256::ZERO;
    genesis.coinbase = Address::ZERO;
    genesis.base_fee_per_gas = Some(base_fee.to::<u128>());
    genesis.excess_blob_gas = None;
    genesis.blob_gas_used = None;
    genesis.number = Some(0);

    let mut alloc = BTreeMap::new();
    let arbos_storage = build_minimal_arbos_storage(chain_id, base_fee);
    if !arbos_storage.is_empty() {
        let acct = GenesisAccount::default()
            .with_nonce(Some(1))
            .with_balance(U256::ZERO)
            .with_code(None)
            .with_storage(Some(arbos_storage));
        alloc.insert(ARBOS_ADDR, acct);
    }
    genesis.alloc = alloc;

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
