use std::collections::BTreeMap;
use std::io::Read;

use alloy_genesis::GenesisAccount;
use alloy_primitives::{hex, Address, Bytes, B256, U256};
use anyhow::{bail, Result};

use crate::data::{SEPOLIA_SECURE_ALLOC_B64, SEPOLIA_SECURE_ALLOC_SHA256};

fn parse_u256_hex(s: &str) -> U256 {
    let mut h = s.trim_start_matches("0x").to_string();
    if h.is_empty() {
        return U256::ZERO;
    }
    if h.len() % 2 == 1 {
        h.insert(0, '0');
    }
    let bytes = hex::decode(&h).unwrap_or_default();
    U256::from_be_slice(&bytes)
}

fn parse_b256_hex(s: &str) -> B256 {
    let h = s.trim_start_matches("0x");
    let bytes = hex::decode(h).unwrap_or_default();
    let mut out = [0u8; 32];
    let copy_len = bytes.len().min(32);
    out[32 - copy_len..].copy_from_slice(&bytes[bytes.len() - copy_len..]);
    B256::from(out)
}

fn decode_embedded_alloc_json() -> Result<serde_json::Value> {
    let b64_raw = SEPOLIA_SECURE_ALLOC_B64.trim();
    if b64_raw.is_empty() {
        bail!("embedded sepolia alloc is empty");
    }
    let b64: String = b64_raw.chars().filter(|c| !c.is_whitespace()).collect();
    let gz = base64::decode(&b64)?;
    let mut decoder = libflate::gzip::Decoder::new(&gz[..])?;
    let mut buf = Vec::new();
    decoder.read_to_end(&mut buf)?;
    let parsed: serde_json::Value = serde_json::from_slice(&buf)?;
    Ok(parsed)
}

pub fn load_sepolia_secure_alloc_accounts() -> Result<BTreeMap<Address, GenesisAccount>> {
    let parsed = decode_embedded_alloc_json()?;
    let mut out = BTreeMap::<Address, GenesisAccount>::new();

    if let Some(obj) = parsed.get("alloc").and_then(|v| v.as_object()) {
        for (addr_hex, v) in obj {
            let addr_bytes = hex::decode(addr_hex.trim_start_matches("0x")).unwrap_or_default();
            if addr_bytes.len() != 20 {
                continue;
            }
            let addr = Address::from_slice(&addr_bytes);

            let nonce = v.get("nonce").and_then(|x| x.as_str()).map(|s| parse_u256_hex(s).to::<u64>());
            let balance = v.get("balance").and_then(|x| x.as_str()).map(parse_u256_hex).unwrap_or(U256::ZERO);
            let code_opt = v.get("code").and_then(|x| x.as_str()).map(|s| {
                let hexbytes = hex::decode(s.trim_start_matches("0x")).unwrap_or_default();
                Bytes::from(hexbytes)
            });

            let mut storage_map = BTreeMap::<B256, B256>::new();
            if let Some(stor) = v.get("storage").and_then(|x| x.as_object()) {
                for (k, val) in stor {
                    let sk = parse_b256_hex(k);
                    let sv = parse_b256_hex(val.as_str().unwrap_or("0x0"));
                    storage_map.insert(sk, sv);
                }
            }

            let mut acct = GenesisAccount::default().with_balance(balance);
            if let Some(n) = nonce {
                acct = acct.with_nonce(Some(n));
            }
            if let Some(code) = code_opt {
                acct = acct.with_code(Some(code));
            }
            if !storage_map.is_empty() {
                acct = acct.with_storage(Some(storage_map));
            }
            out.insert(addr, acct);
        }
        return Ok(out);
    }

    if parsed.get("secureAlloc").and_then(|v| v.as_object()).is_some() {
        bail!("embedded alloc uses secure keys; need plain addresses");
    }

    bail!("unexpected embedded alloc format")
}

use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::keccak256;
use reth_primitives_traits::Account;
use reth_trie_common::HashedStorage;

pub fn load_sepolia_secure_alloc_hashed(
) -> Result<(BTreeMap<B256, Account>, BTreeMap<B256, HashedStorage>)> {
    let (accounts_h, storages_h, _bytecodes) = load_sepolia_secure_alloc_hashed_with_bytecodes()?;
    Ok((accounts_h, storages_h))
}

/// Load the embedded Sepolia genesis alloc with accounts, storage, and bytecodes.
/// Returns (hashed_accounts, hashed_storage, bytecodes)
/// The bytecodes map is keyed by code hash (B256) with value being the raw bytecode bytes.
pub fn load_sepolia_secure_alloc_hashed_with_bytecodes(
) -> Result<(BTreeMap<B256, Account>, BTreeMap<B256, HashedStorage>, BTreeMap<B256, Vec<u8>>)> {
    let parsed = decode_embedded_alloc_json()?;
    let mut accounts_h = BTreeMap::<B256, Account>::new();
    let mut storages_h = BTreeMap::<B256, HashedStorage>::new();
    let mut bytecodes = BTreeMap::<B256, Vec<u8>>::new();

    let obj = parsed.get("secureAlloc").and_then(|v| v.as_object()).ok_or_else(|| {
        anyhow::anyhow!("secureAlloc not found in embedded artifact")
    })?;

    for (hashed_addr_hex, v) in obj {
        let hashed_addr = parse_b256_hex(hashed_addr_hex);

        let balance = v
            .get("balance")
            .and_then(|x| x.as_str())
            .map(parse_u256_hex)
            .unwrap_or(U256::ZERO);
        let nonce_u256 = v.get("nonce").and_then(|x| x.as_str()).map(parse_u256_hex).unwrap_or(U256::ZERO);
        let nonce = nonce_u256.to::<u64>();

        let code_bytes = v.get("code").and_then(|x| x.as_str()).map(|s| hex::decode(s.trim_start_matches("0x")).unwrap_or_default()).unwrap_or_default();
        let code_hash = if !code_bytes.is_empty() {
            let hash = keccak256(&code_bytes);
            // Store the bytecode keyed by its hash
            bytecodes.insert(hash, code_bytes);
            hash
        } else if let Some(ch) = v.get("codeHash").and_then(|x| x.as_str()) {
            parse_b256_hex(ch)
        } else {
            KECCAK_EMPTY
        };

        let acct = Account { nonce, balance, bytecode_hash: Some(code_hash) };
        accounts_h.insert(hashed_addr, acct);

        let mut storage_entries: BTreeMap<B256, U256> = BTreeMap::new();
        if let Some(stor) = v.get("storage").and_then(|x| x.as_object()) {
            for (k, val) in stor {
                let sk = parse_b256_hex(k);
                let sv = parse_u256_hex(val.as_str().unwrap_or("0x0"));
                storage_entries.insert(sk, sv);
            }
        }
        if !storage_entries.is_empty() {
            let storage = HashedStorage::from_iter(false, storage_entries.into_iter());
            storages_h.insert(hashed_addr, storage);
        }
    }

    // Add missing TimeoutQueue slot 1 (nextGetOffset=2) to ArbOS storage
    // Go nitro's InitializeQueue sets both nextPutOffset and nextGetOffset to 2
    // The secureAlloc data is missing slot 1, so we add it here
    let arbos_hashed_addr = B256::from_slice(&hex::decode("b4d14ec89c201c23aa60e231e3993b3966b33ff1f55d198ec25980957ab32065").unwrap());
    // TimeoutQueue slot 1 hashed key = keccak256(preimage_slot_1)
    // where preimage_slot_1 = 0x9e9ffd355c04cc0ffaba550b5b46d79f750513bcaf322e22daca18080c857a01
    let timeout_queue_slot1_hashed = B256::from_slice(&hex::decode("3685c8c6988ac7abfacfa36241f4ee3b2c9fd55a665e86b9bf8b4fa0130799ca").unwrap());
    
    // DEBUG: Log before adding the slot
    eprintln!("DEBUG embedded_alloc: Adding TimeoutQueue slot 1 to ArbOS storage");
    eprintln!("DEBUG embedded_alloc: ArbOS hashed addr: {:?}", arbos_hashed_addr);
    eprintln!("DEBUG embedded_alloc: storages_h contains ArbOS: {}", storages_h.contains_key(&arbos_hashed_addr));
    
    if let Some(arbos_storage) = storages_h.get_mut(&arbos_hashed_addr) {
        // Add the missing slot to existing storage
        eprintln!("DEBUG embedded_alloc: ArbOS storage exists, adding to existing storage");
        let mut entries: BTreeMap<B256, U256> = arbos_storage.storage.iter().map(|(k, v)| (*k, *v)).collect();
        entries.insert(timeout_queue_slot1_hashed, U256::from(2));
        *arbos_storage = HashedStorage::from_iter(false, entries.into_iter());
    } else {
        // ArbOS storage doesn't exist in secureAlloc, create it with just the TimeoutQueue slot 1
        eprintln!("DEBUG embedded_alloc: ArbOS storage does NOT exist, creating new storage entry");
        let mut entries: BTreeMap<B256, U256> = BTreeMap::new();
        entries.insert(timeout_queue_slot1_hashed, U256::from(2));
        let storage = HashedStorage::from_iter(false, entries.into_iter());
        storages_h.insert(arbos_hashed_addr, storage);
        eprintln!("DEBUG embedded_alloc: storages_h now has {} entries", storages_h.len());
    }

    Ok((accounts_h, storages_h, bytecodes))
}
