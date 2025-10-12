#![cfg_attr(not(feature = "std"), no_std)]

use alloy_genesis::{Genesis, GenesisAccount};
use reth_chainspec::EthChainSpec;
use alloy_primitives::{hex, keccak256, Address, Bytes, B256, U256};
use anyhow::Result;
use reth_chainspec::ChainSpec;
mod data;
#[cfg(feature = "std")]
pub mod embedded_alloc;

use reth_cli::chainspec::{parse_genesis, ChainSpecParser};
use std::sync::Arc;

use alloy_chains::Chain;
use revm::primitives::hardfork::SpecId;

extern crate alloc;

use alloc::collections::BTreeMap;

const ARBOS_ADDR: Address =
    alloy_primitives::address!("0xA4B05FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF");

fn insert_non_zero(storage: &mut BTreeMap<B256, B256>, key: B256, val: B256) {
    if val != B256::ZERO {
        storage.insert(key, val);
    }
}

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
const BATCH_POSTER_TABLE_KEY: u8 = 0;
const POSTER_ADDRS_KEY: u8 = 0;
const POSTER_INFO_KEY: u8 = 1;

const L1_PAY_REWARDS_TO_OFFSET: u64 = 0;
const L1_EQUIL_UNITS_OFFSET: u64 = 1;
const L1_INERTIA_OFFSET: u64 = 2;
const L1_PER_UNIT_REWARD_OFFSET: u64 = 3;
const L1_LAST_UPDATE_TIME_OFFSET: u64 = 4;
const L1_FUNDS_DUE_FOR_REWARDS_OFFSET: u64 = 5;
const L1_UNITS_SINCE_OFFSET: u64 = 6;
const L1_PRICE_PER_UNIT_OFFSET: u64 = 7;
const L1_LAST_SURPLUS_OFFSET: u64 = 8;
const L1_PER_BATCH_GAS_COST_OFFSET: u64 = 9;
const L1_AMORTIZED_COST_CAP_BIPS_OFFSET: u64 = 10;
const L1_FEES_AVAILABLE_OFFSET: u64 = 11;

const BPT_TOTAL_FUNDS_DUE_OFFSET: u64 = 0;

const INITIAL_L1_INITIAL_INERTIA: u64 = 10;
const INITIAL_L1_INITIAL_PER_UNIT_REWARD: u64 = 10;
const INITIAL_EQUIL_UNITS_V0: u64 = 60 * 16 * 100_000;

const BATCH_POSTER: Address =
    alloy_primitives::address!("0xA4B000000000000000000073657175656e636572");
const BATCH_POSTER_PAYTO: Address = BATCH_POSTER;
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
const INITIAL_SPEED_LIMIT_PER_SECOND_V6: u64 = 7_000_000;
const INITIAL_PER_BLOCK_GAS_LIMIT_V6: u64 = 32_000_000;
const INITIAL_EQUIL_UNITS_V6: u64 = 16 * 10_000_000;

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
            while j < buf.len()
                && (buf[j] == b' '
                    || buf[j] == b'\t'
                    || buf[j] == b'\r'
                    || buf[j] == b'\n'
                    || buf[j] == b':')
            {
                j += 1;
            }
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
            if any {
                return Some(val);
            }
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
            while j < buf.len()
                && (buf[j] == b' '
                    || buf[j] == b'\t'
                    || buf[j] == b'\r'
                    || buf[j] == b'\n'
                    || buf[j] == b':')
            {
                j += 1;
            }
            if j + 2 >= buf.len() || buf[j] != b'"' {
                return None;
            }
            j += 1;
            if j + 2 >= buf.len() || buf[j] != b'0' || buf[j + 1] != b'x' {
                return None;
            }
            j += 2;
            let start = j;
            let mut bytes = [0u8; 20];
            let mut k = 0usize;
            while j < buf.len() && k < 20 {
                if j + 1 >= buf.len() {
                    break;
                }
                let hi = match hex_char_val(buf[j]) {
                    Some(v) => v,
                    None => break,
                };
                let lo = match hex_char_val(buf[j + 1]) {
                    Some(v) => v,
                    None => break,
                };
                bytes[k] = (hi << 4) | lo;
                k += 1;
                j += 2;
            }
            while j < buf.len() && buf[j] != b'"' {
                j += 1;
            }
            if k == 20 {
                return Some(Address::from_slice(&bytes));
            } else {
                let hex_len = j.saturating_sub(start);
                if hex_len > 0 && hex_len <= 40 {
                    let mut tmp = [0u8; 20];
                    let mut bi = 20usize;
                    let mut p = start + hex_len;
                    while p > start && bi > 0 {
                        let lo = if p > start {
                            p -= 1;
                            hex_char_val(buf[p]).unwrap_or(0)
                        } else {
                            0
                        };
                        let hi = if p > start {
                            p -= 1;
                            hex_char_val(buf[p]).unwrap_or(0)
                        } else {
                            0
                        };
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

fn subspace_bytes(storage_key: &[u8], id: &[u8]) -> Vec<u8> {
    keccak256([storage_key, id].concat()).to_vec()
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
        let slot = map_slot(storage_key, be_u64(offset));
        let val = B256::from(last);
        #[cfg(feature = "std")]
        {
            eprintln!(
                "DBG write_bytes: len={} last_idx={} slot=0x{} val=0x{}",
                bytes.len(),
                offset,
                hex::encode(slot.as_slice()),
                hex::encode(val.as_slice())
            );
        }
        storage.insert(slot, val);
    }
}
fn minify_json_preserve(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut in_str = false;
    let mut esc = false;
    for &b in input {
        if in_str {
            out.push(b);
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
        } else {
            match b {
                b'"' => {
                    in_str = true;
                    out.push(b);
                }
                b' ' | b'\n' | b'\r' | b'\t' => {}
                _ => out.push(b),
            }
        }
    }
    out
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
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_SPEED_LIMIT_PER_SECOND_OFFSET)),
        be_u64(INITIAL_SPEED_LIMIT_PER_SECOND_V6),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_PER_BLOCK_GAS_LIMIT_OFFSET)),
        be_u64(INITIAL_PER_BLOCK_GAS_LIMIT_V6),
    );
    let initial_l2_base_fee = U256::from(87_500_000u64);
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_BASE_FEE_WEI_OFFSET)),
        be_u256(initial_l2_base_fee),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_MIN_BASE_FEE_WEI_OFFSET)),
        be_u256(U256::from(INITIAL_MIN_BASE_FEE_WEI)),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_PRICING_INERTIA_OFFSET)),
        be_u64(INITIAL_PRICING_INERTIA),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_GAS_BACKLOG_OFFSET)),
        be_u64(0),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_BACKLOG_TOLERANCE_OFFSET)),
        be_u64(INITIAL_BACKLOG_TOLERANCE),
    );

    let l1_space = subspace(&root_key, L1_PRICING_SUBSPACE);
    insert_non_zero(
        &mut storage,
        map_slot(&l1_space, be_u64(L1_PRICE_PER_UNIT_OFFSET)),
        be_u256(initial_l1_base_fee),
    );

    storage
}

pub fn build_full_arbos_storage(
    chain_id: u64,
    chain_config_bytes: Option<Bytes>,
    initial_l1_base_fee: U256,
) -> BTreeMap<B256, B256> {
    let mut storage = BTreeMap::<B256, B256>::new();
    let root_key: Vec<u8> = Vec::new();

    let mut desired_arbos_version = 1u64;
    let mut initial_chain_owner = Address::ZERO;
    let mut genesis_block_num = 0u64;

    if let Some(cfg) = chain_config_bytes.clone() {
        let cc_space = subspace(&root_key, CHAIN_CONFIG_SUBSPACE);
        let raw = cfg.as_ref();
        write_bytes(&mut storage, &cc_space, raw);

        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(raw) {
            let mut obj = &v;
            if let Some(arb) = v.get("arbitrum") {
                obj = arb;
            }
            if let Some(n) = obj.get("InitialArbOSVersion").and_then(|x| x.as_u64()) {
                desired_arbos_version = n;
            } else if let Some(n) = v.get("InitialArbOSVersion").and_then(|x| x.as_u64()) {
                desired_arbos_version = n;
            }
            if let Some(owner_str) = obj
                .get("InitialChainOwner")
                .and_then(|x| x.as_str())
                .or_else(|| v.get("InitialChainOwner").and_then(|x| x.as_str()))
            {
                if let Ok(bytes) = hex::decode(owner_str.trim_start_matches("0x")) {
                    if bytes.len() == 20 {
                        initial_chain_owner = Address::from_slice(&bytes);
                    }
                }
            }
            if let Some(n) = obj
                .get("GenesisBlockNum")
                .and_then(|x| x.as_u64())
                .or_else(|| v.get("GenesisBlockNum").and_then(|x| x.as_u64()))
            {
                genesis_block_num = n;
            }
        } else {
            let buf = raw;
            if let Some(v) = find_json_number(buf, br#""InitialArbOSVersion""#) {
                desired_arbos_version = v;
            }
            if let Some(owner) = find_json_address(buf, br#""InitialChainOwner""#) {
                initial_chain_owner = owner;
            }
            if let Some(n) = find_json_number(buf, br#""GenesisBlockNum""#) {
                genesis_block_num = n;
            }
        }
    }

    storage.insert(map_slot(&root_key, be_u64(VERSION_OFFSET)), be_u64(desired_arbos_version));
    storage.insert(map_slot(&root_key, be_u64(UPGRADE_VERSION_OFFSET)), be_u64(0));
    storage.insert(map_slot(&root_key, be_u64(UPGRADE_TIMESTAMP_OFFSET)), be_u64(0));
    if desired_arbos_version >= 2 {
        storage.insert(
            map_slot(&root_key, be_u64(NETWORK_FEE_ACCOUNT_OFFSET)),
            address_to_b256(initial_chain_owner),
        );
    } else {
        storage.insert(map_slot(&root_key, be_u64(NETWORK_FEE_ACCOUNT_OFFSET)), B256::ZERO);
    }
    storage.insert(map_slot(&root_key, be_u64(CHAIN_ID_OFFSET)), be_u256(U256::from(chain_id)));
    storage
        .insert(map_slot(&root_key, be_u64(GENESIS_BLOCK_NUM_OFFSET)), be_u64(genesis_block_num));
    storage.insert(map_slot(&root_key, be_u64(INFRA_FEE_ACCOUNT_OFFSET)), B256::ZERO);
    storage.insert(map_slot(&root_key, be_u64(BROTLI_LEVEL_OFFSET)), be_u64(0));
    storage.insert(map_slot(&root_key, be_u64(NATIVE_TOKEN_ENABLED_FROM_TIME_OFFSET)), be_u64(0));

    let l2_space = subspace(&root_key, L2_PRICING_SUBSPACE);
    let l1_space = subspace(&root_key, L1_PRICING_SUBSPACE);

    let (l2_speed_limit, l2_per_block_limit) = if desired_arbos_version >= 6 {
        (INITIAL_SPEED_LIMIT_PER_SECOND_V6, INITIAL_PER_BLOCK_GAS_LIMIT_V6)
    } else {
        (INITIAL_SPEED_LIMIT_PER_SECOND_V0, INITIAL_PER_BLOCK_GAS_LIMIT_V0)
    };
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_SPEED_LIMIT_PER_SECOND_OFFSET)),
        be_u64(l2_speed_limit),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_PER_BLOCK_GAS_LIMIT_OFFSET)),
        be_u64(l2_per_block_limit),
    );
    let initial_l2_base_fee = U256::from(87_500_000u64);
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_BASE_FEE_WEI_OFFSET)),
        be_u256(initial_l2_base_fee),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_GAS_BACKLOG_OFFSET)),
        be_u64(0),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_MIN_BASE_FEE_WEI_OFFSET)),
        be_u256(U256::from(INITIAL_MIN_BASE_FEE_WEI)),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_PRICING_INERTIA_OFFSET)),
        be_u64(INITIAL_PRICING_INERTIA),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_BACKLOG_TOLERANCE_OFFSET)),
        be_u64(INITIAL_BACKLOG_TOLERANCE),
    );

    let initial_rewards_recipient =
        if desired_arbos_version >= 2 { initial_chain_owner } else { BATCH_POSTER_PAYTO };
    insert_non_zero(
        &mut storage,
        map_slot(&l1_space, be_u64(L1_PAY_REWARDS_TO_OFFSET)),
        address_to_b256(initial_rewards_recipient),
    );
    let l1_equil_units =
        if desired_arbos_version >= 6 { INITIAL_EQUIL_UNITS_V6 } else { INITIAL_EQUIL_UNITS_V0 };
    insert_non_zero(
        &mut storage,
        map_slot(&l1_space, be_u64(L1_EQUIL_UNITS_OFFSET)),
        be_u64(l1_equil_units),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l1_space, be_u64(L1_INERTIA_OFFSET)),
        be_u64(INITIAL_L1_INITIAL_INERTIA),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l1_space, be_u64(L1_PER_UNIT_REWARD_OFFSET)),
        be_u64(INITIAL_L1_INITIAL_PER_UNIT_REWARD),
    );
    storage.insert(map_slot(&l1_space, be_u64(L1_PER_BATCH_GAS_COST_OFFSET)), be_u64(100_000));
    insert_non_zero(
        &mut storage,
        map_slot(&l1_space, be_u64(L1_AMORTIZED_COST_CAP_BIPS_OFFSET)),
        be_u64(u64::MAX),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l1_space, be_u64(L1_LAST_UPDATE_TIME_OFFSET)),
        be_u64(0),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l1_space, be_u64(L1_FUNDS_DUE_FOR_REWARDS_OFFSET)),
        B256::ZERO,
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l1_space, be_u64(L1_UNITS_SINCE_OFFSET)),
        be_u64(0),
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l1_space, be_u64(L1_LAST_SURPLUS_OFFSET)),
        B256::ZERO,
    );
    insert_non_zero(
        &mut storage,
        map_slot(&l1_space, be_u64(L1_FEES_AVAILABLE_OFFSET)),
        B256::ZERO,
    );

    let bpt_space = subspace(&l1_space, BATCH_POSTER_TABLE_KEY);
    insert_non_zero(
        &mut storage,
        map_slot(&bpt_space, be_u64(BPT_TOTAL_FUNDS_DUE_OFFSET)),
        be_u64(0),
    );

    let poster_addrs_space = subspace(&bpt_space, POSTER_ADDRS_KEY);
    insert_non_zero(&mut storage, map_slot(&poster_addrs_space, be_u64(0)), be_u64(1));
    insert_non_zero(
        &mut storage,
        map_slot(&poster_addrs_space, be_u64(1)),
        address_to_b256(BATCH_POSTER),
    );
    let poster_byaddr_space = subspace(&poster_addrs_space, 0);
    insert_non_zero(
        &mut storage,
        map_slot(&poster_byaddr_space, address_to_b256(BATCH_POSTER)),
        be_u64(1),
    );

    let poster_info_space = subspace(&bpt_space, POSTER_INFO_KEY);


    insert_non_zero(
        &mut storage,
        map_slot(&l2_space, be_u64(L2_BACKLOG_TOLERANCE_OFFSET)),
        be_u64(INITIAL_BACKLOG_TOLERANCE),
    );
    let per_poster_space = subspace_bytes(&poster_info_space, BATCH_POSTER.as_slice());
    insert_non_zero(
        &mut storage,
        map_slot(&per_poster_space, be_u64(1)),
        address_to_b256(BATCH_POSTER_PAYTO),
    );

    let price_per_unit_offset: u64 = 7;
    insert_non_zero(
        &mut storage,
        map_slot(&l1_space, be_u64(price_per_unit_offset)),
        be_u256(initial_l1_base_fee),
    );

    let chain_owner_space = subspace(&root_key, CHAIN_OWNER_SUBSPACE);
    storage.insert(map_slot(&chain_owner_space, be_u64(0)), be_u64(1));
    storage.insert(map_slot(&chain_owner_space, be_u64(1)), address_to_b256(initial_chain_owner));
    let native_token_owner_space = subspace(&root_key, 10);
    insert_non_zero(&mut storage, map_slot(&native_token_owner_space, be_u64(0)), be_u64(0));

    let byaddr = subspace(&chain_owner_space, 0);
    storage.insert(map_slot(&byaddr, address_to_b256(initial_chain_owner)), be_u64(1));

    if desired_arbos_version >= 30 {
        let programs_space = subspace(&root_key, 8);
        let params_space = subspace(&programs_space, 0);
        let mut stylus_bytes: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        let mut push_be_n = |val: u64, n: usize| {
            let be = val.to_be_bytes();
            stylus_bytes.extend_from_slice(&be[8 - n..]);
        };
        push_be_n(1, 2);
        push_be_n(10_000, 3);
        push_be_n(262_144, 4);
        push_be_n(2, 2);
        push_be_n(1000, 2);
        push_be_n(128, 2);
        push_be_n(72, 1);
        push_be_n(11, 1);
        push_be_n(50, 1);
        push_be_n(50, 1);
        push_be_n(365, 2);
        push_be_n(31, 2);
        push_be_n(32, 2);
        let mut slot_idx: u64 = 0;
        let mut i = 0usize;
        while i < stylus_bytes.len() {
            let end = core::cmp::min(i + 32, stylus_bytes.len());
            let mut buf = [0u8; 32];
            let chunk = &stylus_bytes[i..end];
            buf[0..chunk.len()].copy_from_slice(chunk);
            storage.insert(map_slot(&params_space, be_u64(slot_idx)), B256::from(buf));
            slot_idx += 1;
            i = end;
        }

        let data_pricer_space = subspace(&programs_space, 3);
        let initial_bytes_per_second: u64 =
            (((1u128 << 40) / (365u128 * 24u128)) / (60u128 * 60u128)) as u64;
        insert_non_zero(
            &mut storage,
            map_slot(&data_pricer_space, be_u64(1)),
            be_u64(initial_bytes_per_second),
        );
        insert_non_zero(
            &mut storage,
            map_slot(&data_pricer_space, be_u64(2)),
            be_u64(1_421_388_000),
        );
        insert_non_zero(&mut storage, map_slot(&data_pricer_space, be_u64(3)), be_u64(82_928_201));
        insert_non_zero(&mut storage, map_slot(&data_pricer_space, be_u64(4)), be_u64(21_360_419));
    }

    storage.insert(map_slot(&l1_space, be_u64(L1_PER_BATCH_GAS_COST_OFFSET)), be_u64(100_000));

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
    state_root_hex: &str,
    gas_limit_hex: &str,
    extra_data_hex: &str,
    mix_hash_hex: &str,
    nonce_hex: &str,
    chain_config_bytes: Option<&[u8]>,
    initial_l1_base_fee_hex: Option<&str>,
) -> Result<ChainSpec> {
    let base_fee = parse_hex_quantity(base_fee_hex);
    let ts = parse_hex_quantity(timestamp_hex);
    let gas_limit = parse_hex_quantity(gas_limit_hex);
    let extra = hex::decode(extra_data_hex.trim_start_matches("0x")).unwrap_or_default();
    let mix_hash = {
        let bytes = hex::decode(mix_hash_hex.trim_start_matches("0x")).unwrap_or_default();
        let mut arr = [0u8; 32];
        let copy = bytes.len().min(32);
        if copy > 0 {
            arr[32 - copy..].copy_from_slice(&bytes[bytes.len() - copy..]);
        }
        B256::from(arr)
    };
    let nonce = parse_hex_quantity(nonce_hex).to::<u64>();
    let state_root = {
        let bytes = hex::decode(state_root_hex.trim_start_matches("0x")).unwrap_or_default();
        let mut arr = [0u8; 32];
        let copy = bytes.len().min(32);
        if copy > 0 {
            arr[32 - copy..].copy_from_slice(&bytes[bytes.len() - copy..]);
        }
        B256::from(arr)
    };

    let mut genesis = Genesis::default();
    genesis.config.chain_id = chain_id;
    genesis.config.london_block = Some(0);
    genesis.config.cancun_time = Some(0);

    genesis.nonce = nonce;
    genesis.timestamp = ts.to::<u64>();
    genesis.extra_data = extra.clone().into();
    genesis.gas_limit = gas_limit.to::<u64>();
    genesis.difficulty = U256::from(1u64);
    genesis.mix_hash = mix_hash;
    genesis.coinbase = Address::ZERO;
    genesis.base_fee_per_gas = Some(base_fee.to::<u128>());
    genesis.excess_blob_gas = None;
    genesis.blob_gas_used = None;
    genesis.number = Some(0);

    let mut alloc = BTreeMap::new();
    #[cfg(feature = "std")]
    {
        if let Ok(embedded) = crate::embedded_alloc::load_sepolia_secure_alloc_accounts() {
            if !embedded.is_empty() {
                alloc = embedded;
            }
        }
    }

    let chain_cfg_bytes = chain_config_bytes.map(|b| alloy_primitives::Bytes::from(b.to_vec()));

    let initial_l1_price = initial_l1_base_fee_hex
        .map(|h| parse_hex_quantity(h))
        .unwrap_or_else(|| U256::from(50_000_000_000u64));

    if alloc.is_empty() {
        let arbos_storage = build_full_arbos_storage(chain_id, chain_cfg_bytes, initial_l1_price);
        if !arbos_storage.is_empty() {
            let acct = GenesisAccount::default()
                .with_nonce(Some(1))
                .with_balance(U256::ZERO)
                .with_code(Some(Bytes::default()))
                .with_storage(Some(arbos_storage));
            alloc.insert(ARBOS_ADDR, acct);
        }

        for &b in &[0x64u8, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6b, 0x6c, 0x6d, 0x6e, 0x6f, 0x70, 0xff] {
            let mut a = [0u8; 20];
            a[19] = b;
            let addr = Address::from_slice(&a);
            let acct = GenesisAccount::default()
                .with_nonce(Some(0))
                .with_balance(U256::ZERO)
                .with_code(Some(Bytes::from_static(&[0xfe])));
            alloc.insert(addr, acct);
        }
        for &b in &[0x71u8, 0x72, 0x73, 0xc8, 0xc9] {
            let mut a = [0u8; 20];
            a[19] = b;
            let addr = Address::from_slice(&a);
            let acct = GenesisAccount::default()
                .with_nonce(Some(0))
                .with_balance(U256::ZERO)
                .with_code(Some(Bytes::default()));
            alloc.insert(addr, acct);
        }
    }

    genesis.alloc = alloc;

    let mut spec = reth_chainspec::ChainSpec::from_genesis(genesis.clone());

    let mut header = alloy_consensus::Header::default();
    header.number = 0u64.into();
    header.gas_limit = gas_limit.to::<u64>();
    header.difficulty = U256::from(1u64);
    header.nonce = nonce.into();
    header.extra_data = extra.into();
    header.state_root = state_root;
    header.timestamp = ts.to::<u64>();
    header.mix_hash = mix_hash;
    header.beneficiary = Address::ZERO;
    header.base_fee_per_gas = Some(base_fee.to::<u64>());

    use reth_primitives_traits::SealedHeader;
    let sealed = SealedHeader::seal_slow(header);
    spec.genesis_header = sealed;
    spec.genesis = genesis;

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

    const SUPPORTED_CHAINS: &'static [&'static str] =
        &["arbitrum-sepolia", "arb-sepolia", "arbsepolia", "421614"];

    fn parse(s: &str) -> eyre::Result<Arc<Self::ChainSpec>> {
        match s {
            "arbitrum-sepolia" | "arb-sepolia" | "arbsepolia" | "421614" => {
                Ok(Arc::new(arbitrum_sepolia_spec()))
            }
            _ => Ok(Arc::new(reth_chainspec::ChainSpec::from_genesis(parse_genesis(s)?))),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives_traits::SealedHeader;

    #[test]
    fn baked_genesis_basic_fields_match_inputs() {
        let chain_id = 421614u64;
        let base_fee_hex = "0x3b9aca00"; // 1_000_000_000
        let timestamp_hex = "0x0";
        let state_root_hex = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let gas_limit_hex = "0x1312d00"; // 20000000
        let extra_data_hex = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let mix_hash_hex = "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let nonce_hex = "0x1";

        let spec = sepolia_baked_genesis_from_header(
            chain_id,
            base_fee_hex,
            timestamp_hex,
            state_root_hex,
            gas_limit_hex,
            extra_data_hex,
            mix_hash_hex,
            nonce_hex,
            None,
            Some("0x2faf080"), // 50_000_000_000
        ).expect("ok");

        let header = spec.genesis_header.header();
        assert_eq!(header.number, 0u64);
        assert_eq!(header.gas_limit, parse_hex_quantity(gas_limit_hex).to::<u64>());
        assert_eq!(header.base_fee_per_gas, Some(parse_hex_quantity(base_fee_hex).to::<u64>()));
        assert_eq!(header.timestamp, parse_hex_quantity(timestamp_hex).to::<u64>());

        let extra = hex::decode(extra_data_hex.trim_start_matches("0x")).unwrap();
        assert_eq!(&header.extra_data[..], &extra[..]);

        let mix_bytes = {
            let bytes = hex::decode(mix_hash_hex.trim_start_matches("0x")).unwrap();
            let mut arr = [0u8; 32];
            let copy = bytes.len().min(32);
            if copy > 0 {
                arr[32 - copy..].copy_from_slice(&bytes[bytes.len() - copy..]);
            }
            B256::from(arr)
        };
        assert_eq!(header.mix_hash, mix_bytes);
        let expected_nonce = alloy_primitives::B64::from(parse_hex_quantity(nonce_hex).to::<u64>().to_be_bytes());
        assert_eq!(header.nonce, expected_nonce);
    }
}
