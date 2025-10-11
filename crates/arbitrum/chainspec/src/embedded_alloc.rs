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
    let parsed = decode_embedded_alloc_json()?;
    let mut accounts_h = BTreeMap::<B256, Account>::new();
    let mut storages_h = BTreeMap::<B256, HashedStorage>::new();

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
            keccak256(&code_bytes)
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

    Ok((accounts_h, storages_h))
}
