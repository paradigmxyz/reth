//! State export utilities for debugging Mantle hardfork issues
//!
//! This module provides functionality to export complete blockchain state to JSON files,
//! extracting original keys from `BundleState` for debugging purposes.

use alloy_primitives::{hex, keccak256, Address, B256, U256};
use eyre::Result;
use reth_db::tables;
use reth_db_api::cursor::{DbCursorRO, DbDupCursorRO};
use reth_db_api::transaction::DbTx;
use reth_provider::DBProvider;
use reth_primitives_traits::Account;
use reth_trie::HashedStorage;
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseStorageRoot, DatabaseTrieCursorFactory};
use revm::database::BundleState;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};

/// Storage entry for JSON export
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct StorageEntryExport {
    /// Original storage key before hashing
    pub original_key: String,
    /// Keccak256 hash of the storage key
    pub hashed_key: String,
    /// Storage slot value
    pub value: String,
}

/// Account data for JSON export
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct AccountExport {
    /// Keccak256 hash of the account address
    pub hashed_address: String,
    /// Account balance in wei
    pub balance: String,
    /// Account nonce (transaction count)
    pub nonce: u64,
    /// Contract bytecode (hex encoded)
    pub code: String,
    /// Keccak256 hash of the contract code
    pub code_hash: String,
    /// Root hash of the account's storage trie
    pub storage_hash: String,
    /// Map of storage entries for this account
    pub storage: BTreeMap<String, StorageEntryExport>,
}

/// Extract storage key mapping from `BundleState` (`hashed_key` -> `original_key`)
fn extract_storage_key_mapping_from_bundle(bundle_state: &BundleState) -> BTreeMap<B256, BTreeMap<B256, B256>> {
    let mut mapping: BTreeMap<B256, BTreeMap<B256, B256>> = BTreeMap::new();
    
    for (address, account) in &bundle_state.state {
        let hashed_address = keccak256(address.as_slice());
        
        for slot in account.storage.keys() {
            let original_key = B256::from(*slot);
            let hashed_key = keccak256(original_key.as_slice());
            
            mapping.entry(hashed_address)
                .or_default()
                .insert(hashed_key, original_key);
        }
    }
    
    mapping
}

/// Extract address mapping from `BundleState` (`hashed_address` -> `original_address`)
fn extract_address_mapping_from_bundle(bundle_state: &BundleState) -> BTreeMap<B256, Address> {
    let mut mapping: BTreeMap<B256, Address> = BTreeMap::new();
    
    for address in bundle_state.state.keys() {
        let hashed_address = keccak256(address.as_slice());
        mapping.insert(hashed_address, *address);
    }
    
    mapping
}

/// Export full state using `BundleState`
///
/// This function exports the complete blockchain state by:
/// 1. Reading all accounts from the database
/// 2. Applying `BundleState` changes on top of database state
/// 3. Extracting original keys from `BundleState` for accurate debugging
/// 4. Writing the result to a JSON file in a standardized format
///
/// # Arguments
/// * `provider` - Database provider for reading state
/// * `bundle_state` - `BundleState` containing execution changes (with original keys)
/// * `filename` - Output filename
/// * `state_root` - Optional state root hash to include in the export. If None, uses zero hash.
/// * `export_bundle_storage_only` - If true, only export storage details for accounts in `bundle_state`
///
/// # Format
/// The output JSON has the following structure:
/// ```json
/// {
///   "state_root": "0x...",
///   "accounts": {
///     "0xAddress": {
///       "hashed_address": "0x...",
///       "balance": "123",
///       "nonce": 0,
///       "code": "0x...",
///       "code_hash": "0x...",
///       "storage_hash": "0x...",
///       "storage": {
///         "0xKey": {
///           "original_key": "0x...",
///           "hashed_key": "0x...",
///           "value": "0x..."
///         }
///       }
///     }
///   }
/// }
/// ```
pub fn export_full_state_with_bundle<Provider>(
    provider: &Provider,
    bundle_state: &BundleState,
    filename: &str,
    state_root: Option<B256>,
    export_bundle_storage_only: bool,
) -> Result<()>
where
    Provider: DBProvider,
{
    tracing::info!(
        target: "mantle_hardfork::debug", 
        filename = %filename, 
        export_bundle_storage_only = export_bundle_storage_only,
        "Starting full state export from bundle_state"
    );

    // Step 1: Collect all accounts (database + bundle)
    let mut all_accounts: BTreeMap<B256, Option<Account>> = BTreeMap::new();
    
    // Read from database
    let mut hashed_account_cursor = provider.tx_ref().cursor_read::<tables::HashedAccounts>()?;
    let mut db_account_count = 0;
    
    if let Some((hashed_address, account_info)) = hashed_account_cursor.first()? {
        all_accounts.insert(hashed_address, Some(account_info));
        db_account_count += 1;
        while let Some((hashed_address, account_info)) = hashed_account_cursor.next()? {
            all_accounts.insert(hashed_address, Some(account_info));
            db_account_count += 1;
        }
    }
    
    tracing::info!(target: "mantle_hardfork::debug", "Loaded {} accounts from database", db_account_count);
    
    // Apply bundle state changes
    for (address, account) in &bundle_state.state {
        let hashed_address = keccak256(address.as_slice());
        if let Some(info) = &account.info {
            all_accounts.insert(hashed_address, Some(Account::from(info.clone())));
        } else {
            all_accounts.insert(hashed_address, None);
        }
    }
    
    tracing::info!(target: "mantle_hardfork::debug", "Applied {} account changes from bundle_state", bundle_state.state.len());
    
    // Remove deleted accounts
    all_accounts.retain(|_, account| account.is_some());
    
    tracing::info!(target: "mantle_hardfork::debug", "Total accounts to export: {}", all_accounts.len());
    
    // Step 2: Collect address mapping (hashed -> original)
    let mut address_mapping: BTreeMap<B256, Address> = BTreeMap::new();
    
    // First from database
    let mut plain_account_cursor = provider.tx_ref().cursor_read::<tables::PlainAccountState>()?;
    if let Some((address, _)) = plain_account_cursor.first()? {
        let hashed = keccak256(address.as_slice());
        address_mapping.insert(hashed, address);
        while let Some((address, _)) = plain_account_cursor.next()? {
            let hashed = keccak256(address.as_slice());
            address_mapping.insert(hashed, address);
        }
    }
    
    // Then from bundle_state (this will override database entries with correct values)
    let bundle_address_mapping = extract_address_mapping_from_bundle(bundle_state);
    address_mapping.extend(bundle_address_mapping);
    
    // Step 3: Collect storage key mapping (for original keys)
    let mut storage_key_mapping: BTreeMap<B256, BTreeMap<B256, B256>> = BTreeMap::new();
    
    // First from database
    let mut plain_storage_cursor = provider.tx_ref().cursor_dup_read::<tables::PlainStorageState>()?;
    let mut current_address: Option<Address> = None;
    
    while let Some((address, storage_entry)) = 
        if current_address.is_none() {
            plain_storage_cursor.first()?
        } else {
            plain_storage_cursor.next_no_dup()?
        }
    {
        current_address = Some(address);
        let hashed_address = keccak256(address.as_slice());
        let hashed_key = keccak256(storage_entry.key.as_slice());
        
        storage_key_mapping.entry(hashed_address)
            .or_default()
            .insert(hashed_key, storage_entry.key);
        
        while let Some((_, storage_entry)) = plain_storage_cursor.next_dup()? {
            let hashed_key = keccak256(storage_entry.key.as_slice());
            storage_key_mapping.entry(hashed_address)
                .or_default()
                .insert(hashed_key, storage_entry.key);
        }
    }
    
    // Then from bundle_state (this will add new keys that aren't in the database yet)
    let bundle_storage_mapping = extract_storage_key_mapping_from_bundle(bundle_state);
    for (hashed_addr, keys) in bundle_storage_mapping {
        storage_key_mapping.entry(hashed_addr)
            .or_default()
            .extend(keys);
    }
    
    tracing::info!(target: "mantle_hardfork::debug", "Collected storage key mappings for {} addresses from bundle_state", storage_key_mapping.len());
    
    // Step 4: Use provided state root or default to zero
    let state_root = state_root.unwrap_or_default();
    tracing::info!(target: "mantle_hardfork::debug", state_root = ?state_root, "Using state root");
    
    // Step 5: Stream write to file
    let file = File::create(filename)?;
    let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, file);
    
    // Write JSON header with actual state root
    write!(writer, "{{\n  \"state_root\": \"0x{}\",\n  \"accounts\": {{\n", hex::encode(state_root.as_slice()))?;
    
    let total_accounts = all_accounts.len();
    let mut processed_count = 0;
    let mut first = true;
    
    // Process each account
    for (hashed_address, account_opt) in all_accounts {
        if let Some(account_info) = account_opt {
            processed_count += 1;
            
            // Get original address first (needed for storage operations)
            let original_address = address_mapping.get(&hashed_address).copied().unwrap_or(Address::ZERO);
            
            // Check if this account is in bundle_state
            let is_bundle_account = bundle_state.state.contains_key(&original_address);
            
            // Determine if we should export storage details for this account
            let should_export_storage = !export_bundle_storage_only || is_bundle_account;
            
            // Collect storage sorted by original_key (not hashed_key)
            // We need two maps: one for sorting by original_key, one for calculating storage root
            let mut storage_by_original_key: BTreeMap<B256, U256> = BTreeMap::new();
            let mut hashed_storage_for_root: BTreeMap<B256, U256> = BTreeMap::new();
            
            // Read from database and convert hashed_key back to original_key
            let mut hashed_storage_cursor = provider.tx_ref().cursor_dup_read::<tables::HashedStorages>()?;
            match hashed_storage_cursor.seek_exact(hashed_address)? {
                Some((found_addr, storage_entry)) if found_addr == hashed_address => {
                    let hashed_key = storage_entry.key;
                    let value = storage_entry.value;
                    
                    // Find original key from mapping
                    let original_key = storage_key_mapping.get(&hashed_address)
                        .and_then(|m| m.get(&hashed_key))
                        .copied()
                        .unwrap_or(hashed_key);
                    
                    if should_export_storage {
                        storage_by_original_key.insert(original_key, value);
                    }
                    hashed_storage_for_root.insert(hashed_key, value);
                    
                    while let Some((_, storage_entry)) = hashed_storage_cursor.next_dup()? {
                        let hashed_key = storage_entry.key;
                        let value = storage_entry.value;
                        
                        let original_key = storage_key_mapping.get(&hashed_address)
                            .and_then(|m| m.get(&hashed_key))
                            .copied()
                            .unwrap_or(hashed_key);
                        
                        if should_export_storage {
                            storage_by_original_key.insert(original_key, value);
                        }
                        hashed_storage_for_root.insert(hashed_key, value);
                    }
                }
                _ => {}
            }
            
            // Apply bundle state changes
            if let Some(bundle_account) = address_mapping.get(&hashed_address)
                .and_then(|addr| bundle_state.state.get(addr))
            {
                // Apply storage changes from bundle
                for (slot, slot_value) in &bundle_account.storage {
                    let original_key = B256::from(*slot);
                    let hashed_key = keccak256(original_key.as_slice());
                    
                    if slot_value.present_value == U256::ZERO {
                        if should_export_storage {
                            storage_by_original_key.remove(&original_key);
                        }
                        hashed_storage_for_root.remove(&hashed_key);
                    } else {
                        if should_export_storage {
                            storage_by_original_key.insert(original_key, slot_value.present_value);
                        }
                        hashed_storage_for_root.insert(hashed_key, slot_value.present_value);
                    }
                }
            }
            
            // Calculate storage root using the merged storage
            // Build HashedStorage from hashed_storage_for_root
            let mut hashed_storage = HashedStorage::new(false);
            for (hashed_key, value) in &hashed_storage_for_root {
                hashed_storage.storage.insert(*hashed_key, *value);
            }
            
            // Calculate storage root with bundle changes included
            let storage_hash = if original_address != Address::ZERO && !hashed_storage_for_root.is_empty() {
                // Use the trait method through fully qualified syntax
                use reth_trie::StorageRoot;
                type StorageRootImpl<'a, TX> = StorageRoot<DatabaseTrieCursorFactory<&'a TX>, DatabaseHashedCursorFactory<&'a TX>>;
                
                match <StorageRootImpl<'_, _> as DatabaseStorageRoot<'_, _>>::overlay_root(
                    provider.tx_ref(),
                    original_address,
                    hashed_storage,
                ) {
                    Ok(root) => format!("0x{}", hex::encode(root.as_slice())),
                    Err(_) => "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421".to_string(),
                }
            } else if hashed_storage_for_root.is_empty() {
                // If storage is empty, use empty root
                "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421".to_string()
            } else {
                // If we don't have the original address, use database calculation
                match reth_trie::StorageRoot::from_tx_hashed(provider.tx_ref(), hashed_address).root() {
                    Ok(root) => format!("0x{}", hex::encode(root.as_slice())),
                    Err(_) => "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421".to_string(),
                }
            };
            
            // Get bytecode
            let (code, code_hash_str) = if let Some(code_hash) = account_info.bytecode_hash {
                if code_hash == B256::ZERO {
                    ("0x".to_string(), format!("0x{}", hex::encode(B256::ZERO.as_slice())))
                } else {
                    match provider.tx_ref().get_by_encoded_key::<tables::Bytecodes>(&code_hash) {
                        Ok(Some(bytecode)) => {
                            (format!("0x{}", hex::encode(bytecode.original_bytes())),
                             format!("0x{}", hex::encode(code_hash.as_slice())))
                        },
                        _ => ("0x".to_string(), format!("0x{}", hex::encode(code_hash.as_slice())))
                    }
                }
            } else {
                ("0x".to_string(), format!("0x{}", hex::encode(alloy_primitives::KECCAK256_EMPTY.as_slice())))
            };
            
            // Format address string (checksum format)
            let address_str = if original_address == Address::ZERO {
                format!("hashed:0x{}", hex::encode(hashed_address.as_slice()))
            } else {
                format!("{:?}", original_address)
            };
            
            // Write account
            if !first {
                writer.write_all(b",\n")?;
            }
            first = false;
            
            writeln!(writer, "    \"{}\": {{", address_str)?;
            writeln!(writer, "      \"hashed_address\": \"0x{}\",", hex::encode(hashed_address.as_slice()))?;
            writeln!(writer, "      \"balance\": \"{}\",", account_info.balance)?;
            writeln!(writer, "      \"nonce\": {},", account_info.nonce)?;
            writeln!(writer, "      \"code\": \"{}\",", code)?;
            writeln!(writer, "      \"code_hash\": \"{}\",", code_hash_str)?;
            writeln!(writer, "      \"storage_hash\": \"{}\",", storage_hash)?;
            write!(writer, "      \"storage\": {{")?;
            
            // Write storage (now sorted by original_key)
            if should_export_storage {
                let mut first_storage = true;
                let storage_is_empty = storage_by_original_key.is_empty();
                for (original_key, value) in storage_by_original_key {
                    let hashed_key = keccak256(original_key.as_slice());
                    
                    if !first_storage {
                        writer.write_all(b",")?;
                    }
                    first_storage = false;
                    
                    writeln!(writer, "\n        \"0x{}\": {{", hex::encode(original_key.as_slice()))?;
                    writeln!(writer, "          \"original_key\": \"0x{}\",", hex::encode(original_key.as_slice()))?;
                    writeln!(writer, "          \"hashed_key\": \"0x{}\",", hex::encode(hashed_key.as_slice()))?;
                    writeln!(writer, "          \"value\": \"0x{}\"", hex::encode(value.to_be_bytes::<32>()))?;
                    write!(writer, "        }}")?;
                }
                
                if !storage_is_empty {
                    writer.write_all(b"\n      ")?;
                }
                writer.write_all(b"}\n    }")?;
            } else {
                // For accounts not in bundle, write empty storage object
                writer.write_all(b"}\n    }")?;
            }
            
            if processed_count % 1000 == 0 {
                tracing::info!(target: "mantle_hardfork::debug", 
                              "Processed {}/{} accounts ({:.1}%)", 
                              processed_count, total_accounts,
                              (processed_count as f64 / total_accounts as f64) * 100.0);
            }
        }
    }
    
    writer.write_all(b"\n  }\n}\n")?;
    writer.flush()?;
    
    tracing::info!(target: "mantle_hardfork::debug", 
                  filename = %filename, 
                  total_accounts = total_accounts,
                  "State export completed successfully");
    
    Ok(())
}
