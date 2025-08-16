#![cfg_attr(not(feature = "std"), no_std)]

use anyhow::Result;
use alloy_primitives::{hex, Address, B256, U256, keccak256, Bytes};
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
const RETRYABLES_SUBSPACE: u8 = 2;
const ADDRESS_TABLE_SUBSPACE: u8 = 3;
const CHAIN_OWNER_SUBSPACE: u8 = 4;
const SEND_MERKLE_SUBSPACE: u8 = 5;
const BLOCKHASHES_SUBSPACE: u8 = 6;
const CHAIN_CONFIG_SUBSPACE: u8 = 7;


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

fn address_to_b256(addr: Address) -> B256 {
    let mut w = [0u8; 32];
    let a = addr.as_slice();
    w[12..32].copy_from_slice(a);
    B256::from(w)
}

fn find_json_number(buf: &[u8], key: &[u8]) -> Option<u64> {
    let mut i = 0usize;
    while i + key.len() < buf.len() {
        if &buf[i..i + key.len()] == key {
            let mut j = i + key.len();
            while j < buf.len() && (buf[j] == b' ' || buf[j] == b'\t' || buf[j] == b'\r' || buf[j] == b'\n' || buf[j] == b':' ) { j += 1; }
            let mut val: u64 = 0;
            let mut any = false;
            while j < buf.len() {
                let c = buf[j];
                if c >= b'0' && c <= b'9' {
                    any = true;
                    val = val.saturating_mul(10).saturating_add((c - b'0') as u64);
                    j += 1;
                } else {
                    break;
                }
            }
            if any { return Some(val); }
            return None;
        }
        i += 1;
    }
    None
}

fn hex_char_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(10 + (c - b'a')),
        b'A'..=b'F' => Some(10 + (c - b'A')),
        _ => None,
    }
}

fn find_json_address(buf: &[u8], key: &[u8]) -> Option<Address> {
    let mut i = 0usize;
    while i + key.len() < buf.len() {
        if &buf[i..i + key.len()] == key {
            let mut j = i + key.len();
            while j < buf.len() && (buf[j] == b' ' || buf[j] == b'\t' || buf[j] == b'\r' || buf[j] == b'\n' || buf[j] == b':' ) { j += 1; }
            if j + 2 >= buf.len() || buf[j] != b'"' { return None; }
            j += 1;
            if j + 2 >= buf.len() || buf[j] != b'0' || buf[j+1] != b'x' { return None; }
            j += 2;
            let start = j;
            let mut bytes = [0u8; 20];
            let mut k = 0usize;
            while j < buf.len() && k < 20 {
                if j + 1 >= buf.len() { break; }
                let hi = match hex_char_val(buf[j]) { Some(v) => v, None => break };
                let lo = match hex_char_val(buf[j+1]) { Some(v) => v, None => break };
                bytes[k] = (hi << 4) | lo;
                k += 1;
                j += 2;
            }
            while j < buf.len() && buf[j] != b'"' { j += 1; }
            if k == 20 {
                return Some(Address::from_slice(&bytes));
            } else {
                let hex_len = j.saturating_sub(start);
                if hex_len > 0 && hex_len <= 40 {
                    let mut tmp = [0u8; 20];
                    let mut bi = 20usize;
                    let mut p = start + hex_len;
                    while p > start && bi > 0 {
                        let lo = if p > start { p -= 1; hex_char_val(buf[p]).unwrap_or(0) } else { 0 };
                        let hi = if p > start { p -= 1; hex_char_val(buf[p]).unwrap_or(0) } else { 0 };
                        bi -= 1;
                        tmp[bi] = (hi << 4) | lo;
                    }
                    return Some(Address::from_slice(&tmp));
                }
            }
            return None;
        }
        i += 1;
    }
    None
}

fn parse_hex_quantity(s: &str) -> U256 {
    let mut h = s.strip_prefix("0x").unwrap_or(s).to_string();
    if h.is_empty() {
        return U256::ZERO;
    }
    if h.len() % 2 == 1 {
        h.insert(0, '0');
    }
    let bytes = hex::decode(&h).unwrap_or_default();
    U256::from_be_slice(&bytes)
}

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
fn write_bytes(storage: &mut BTreeMap<B256, B256>, storage_key: &[u8], bytes: &[u8]) {
    storage.insert(map_slot(storage_key, be_u64(0)), be_u64(bytes.len() as u64));
    let mut offset = 1u64;
    let mut i = 0usize;
    while i + 32 <= bytes.len() {
        let word = B256::from_slice(&bytes[i..i + 32]);
        storage.insert(map_slot(storage_key, be_u64(offset)), word);
        offset += 1;
        i += 32;
    }
    if i < bytes.len() {
        let mut last = [0u8; 32];
        let rem = &bytes[i..];
        last[32 - rem.len()..].copy_from_slice(rem);
        storage.insert(map_slot(storage_key, be_u64(offset)), B256::from(last));
    }
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

pub fn build_full_arbos_storage(
    chain_id: u64,
    chain_config_bytes: Option<Bytes>,
    initial_l1_base_fee: U256,
) -> BTreeMap<B256, B256> {
    let mut storage = BTreeMap::<B256, B256>::new();
    let root_key: Vec<u8> = Vec::new();

    let mut arbos_version = 1u64;
    let mut initial_chain_owner = Address::ZERO;

    if let Some(cfg) = chain_config_bytes.clone() {
        let cc_space = subspace(&root_key, CHAIN_CONFIG_SUBSPACE);
        write_bytes(&mut storage, &cc_space, &cfg);

        let buf = cfg.as_ref();
        if let Some(v) = find_json_number(buf, br#""InitialArbOSVersion""#) {
            arbos_version = v;
        }
        if let Some(owner) = find_json_address(buf, br#""InitialChainOwner""#) {
            initial_chain_owner = owner;
        }
    }

    storage.insert(map_slot(&root_key, be_u64(VERSION_OFFSET)), be_u64(arbos_version));
    storage.insert(map_slot(&root_key, be_u64(UPGRADE_VERSION_OFFSET)), be_u64(0));
    storage.insert(map_slot(&root_key, be_u64(UPGRADE_TIMESTAMP_OFFSET)), be_u64(0));
    storage.insert(map_slot(&root_key, be_u64(NETWORK_FEE_ACCOUNT_OFFSET)), address_to_b256(initial_chain_owner));
    storage.insert(map_slot(&root_key, be_u64(CHAIN_ID_OFFSET)), be_u256(U256::from(chain_id)));
    storage.insert(map_slot(&root_key, be_u64(GENESIS_BLOCK_NUM_OFFSET)), be_u64(0));
    storage.insert(map_slot(&root_key, be_u64(INFRA_FEE_ACCOUNT_OFFSET)), B256::ZERO);
    storage.insert(map_slot(&root_key, be_u64(BROTLI_LEVEL_OFFSET)), be_u64(0));
    storage.insert(map_slot(&root_key, be_u64(NATIVE_TOKEN_ENABLED_FROM_TIME_OFFSET)), be_u64(0));

    let l2_space = subspace(&root_key, L2_PRICING_SUBSPACE);
    storage.insert(map_slot(&l2_space, be_u64(L2_SPEED_LIMIT_PER_SECOND_OFFSET)), be_u64(INITIAL_SPEED_LIMIT_PER_SECOND_V0));
    storage.insert(map_slot(&l2_space, be_u64(L2_PER_BLOCK_GAS_LIMIT_OFFSET)), be_u64(INITIAL_PER_BLOCK_GAS_LIMIT_V0));
    storage.insert(map_slot(&l2_space, be_u64(L2_BASE_FEE_WEI_OFFSET)), be_u256(U256::from(INITIAL_MIN_BASE_FEE_WEI)));
    storage.insert(map_slot(&l2_space, be_u64(L2_MIN_BASE_FEE_WEI_OFFSET)), be_u256(U256::from(INITIAL_MIN_BASE_FEE_WEI)));
    storage.insert(map_slot(&l2_space, be_u64(L2_GAS_BACKLOG_OFFSET)), be_u64(0));
    storage.insert(map_slot(&l2_space, be_u64(L2_PRICING_INERTIA_OFFSET)), be_u64(INITIAL_PRICING_INERTIA));
    storage.insert(map_slot(&l2_space, be_u64(L2_BACKLOG_TOLERANCE_OFFSET)), be_u64(INITIAL_BACKLOG_TOLERANCE));

    let l1_space = subspace(&root_key, L1_PRICING_SUBSPACE);
    let price_per_unit_offset: u64 = 7;
    storage.insert(map_slot(&l1_space, be_u64(price_per_unit_offset)), be_u256(initial_l1_base_fee));

    let chain_owner_space = subspace(&root_key, CHAIN_OWNER_SUBSPACE);
    if initial_chain_owner != Address::ZERO {
        storage.insert(map_slot(&chain_owner_space, be_u64(0)), be_u64(1));
        storage.insert(map_slot(&chain_owner_space, be_u64(1)), address_to_b256(initial_chain_owner));
        let byaddr = subspace(&chain_owner_space, 0);
        storage.insert(map_slot(&byaddr, address_to_b256(initial_chain_owner)), be_u64(1));
    } else {
        storage.insert(map_slot(&chain_owner_space, be_u64(0)), be_u64(0));
    }

    let _retryables = subspace(&root_key, RETRYABLES_SUBSPACE);
    let _addr_table = subspace(&root_key, ADDRESS_TABLE_SUBSPACE);
    let _send_merkle = subspace(&root_key, SEND_MERKLE_SUBSPACE);
    let _blockhashes = subspace(&root_key, BLOCKHASHES_SUBSPACE);

    storage
}
 
pub fn sepolia_baked_genesis_from_header(
    chain_id: u64,
    base_fee_hex: &str,
    timestamp_hex: &str,
    _state_root_hex: &str,
    gas_limit_hex: &str,
    extra_data_hex: &str,
    chain_config_json: Option<&str>,
) -> Result<ChainSpec> {
    let base_fee = parse_hex_quantity(base_fee_hex);
    let ts = parse_hex_quantity(timestamp_hex);
    let gas_limit = parse_hex_quantity(gas_limit_hex);
    let extra = hex::decode(extra_data_hex.trim_start_matches("0x")).unwrap_or_default();

    let mut genesis = Genesis::default();
    genesis.config.chain_id = chain_id;
    genesis.config.london_block = Some(0);
    genesis.config.cancun_time = Some(0);

    genesis.nonce = 1;
    genesis.timestamp = ts.to::<u64>();
    genesis.extra_data = extra.into();
    genesis.gas_limit = gas_limit.to::<u64>();
    genesis.difficulty = U256::from(1u64);
    genesis.mix_hash = B256::ZERO;
    genesis.coinbase = Address::ZERO;
    genesis.base_fee_per_gas = Some(base_fee.to::<u128>());
    genesis.excess_blob_gas = None;
    genesis.blob_gas_used = None;
    genesis.number = Some(0);

    let mut alloc = BTreeMap::new();
    let chain_cfg_bytes = chain_config_json.map(|s| alloy_primitives::Bytes::from(s.as_bytes().to_vec()));
    let arbos_storage = build_full_arbos_storage(chain_id, chain_cfg_bytes, base_fee);
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
