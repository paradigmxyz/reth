//! Example demonstrating how to extract the full state of a specific contract from the reth
//! database.
//!
//! This example shows how to:
//! 1. Connect to a reth database
//! 2. Get basic account information (balance, nonce, code hash)
//! 3. Get contract bytecode
//! 4. Iterate through all storage slots for the contract

use alloy_primitives::{keccak256, map::B256Map, Address, U256};
use reth_ethereum::{
    chainspec::ChainSpecBuilder,
    node::EthereumNode,
    primitives::{Account, Bytecode},
    provider::{
        db::{
            cursor::{DbCursorRO, DbDupCursorRO},
            tables,
            transaction::DbTx,
        },
        providers::ReadOnlyConfig,
        ProviderResult,
    },
    storage::{DBProvider, StateProvider, StorageSettingsCache},
};
use std::str::FromStr;

/// Represents the complete state of a contract including account info, bytecode, and storage
#[derive(Debug, Clone)]
pub struct ContractState {
    /// The address of the contract
    pub address: Address,
    /// Basic account information (balance, nonce, code hash)
    pub account: Account,
    /// Contract bytecode (None if not a contract or doesn't exist)
    pub bytecode: Option<Bytecode>,
    /// All storage slots for the contract. Keyed by `keccak256(slot)` when
    /// `hashed` is true, by the raw slot otherwise -- see `hashed`.
    pub storage: B256Map<U256>,
    /// Whether `storage` is keyed by `keccak256(slot)` rather than the raw
    /// slot. True when the node keeps only hashed state (no `PlainStorageState`
    /// mirror), which some storage settings/configurations use to save space.
    pub hashed: bool,
}

/// Extract the full state of a specific contract.
///
/// A node keeps either `PlainStorageState` (keyed by the raw 32-byte slot)
/// or `HashedStorages` (keyed by `keccak256(slot)`) up to date, depending on
/// `cached_storage_settings().use_hashed_state()` -- this is exactly what
/// `StateProvider::storage()` itself branches on internally
/// (`providers/state/latest.rs`). Reading the wrong one doesn't error, it
/// just silently returns zero rows, since the table simply isn't maintained
/// in that mode. Hashed mode has no plain-key mirror to recover the raw slot
/// number from (that's the space it saves), so in that case this returns
/// `keccak256(slot) -> value` pairs instead of `slot -> value`.
pub fn extract_contract_state<P: DBProvider + StorageSettingsCache>(
    provider: &P,
    state_provider: &dyn StateProvider,
    contract_address: Address,
) -> ProviderResult<Option<ContractState>> {
    let account = state_provider.basic_account(&contract_address)?;
    let Some(account) = account else {
        return Ok(None);
    };

    let bytecode = state_provider.account_code(&contract_address)?;

    let hashed = provider.cached_storage_settings().use_hashed_state();
    let mut storage = B256Map::default();

    if hashed {
        let hashed_address = keccak256(contract_address);
        let mut cursor = provider.tx_ref().cursor_dup_read::<tables::HashedStorages>()?;
        if let Some((_, first_entry)) = cursor.seek_exact(hashed_address)? {
            storage.insert(first_entry.key, first_entry.value);
            while let Some((_, entry)) = cursor.next_dup()? {
                storage.insert(entry.key, entry.value);
            }
        }
    } else {
        let mut cursor = provider.tx_ref().cursor_dup_read::<tables::PlainStorageState>()?;
        if let Some((_, first_entry)) = cursor.seek_exact(contract_address)? {
            storage.insert(first_entry.key, first_entry.value);
            while let Some((_, entry)) = cursor.next_dup()? {
                storage.insert(entry.key, entry.value);
            }
        }
    }

    Ok(Some(ContractState { address: contract_address, account, bytecode, storage, hashed }))
}

fn main() -> eyre::Result<()> {
    let address = std::env::var("CONTRACT_ADDRESS")?;
    let contract_address = Address::from_str(&address)?;

    let datadir = std::env::var("RETH_DATADIR")?;
    let spec = ChainSpecBuilder::mainnet().build();
    let runtime = reth_ethereum::tasks::Runtime::test();
    let factory = EthereumNode::provider_factory_builder().open_read_only(
        spec.into(),
        ReadOnlyConfig::from_datadir(datadir),
        runtime,
    )?;

    let provider = factory.provider()?;
    let state_provider = factory.latest()?;
    let contract_state =
        extract_contract_state(&provider, state_provider.as_ref(), contract_address)?;

    if let Some(state) = contract_state {
        println!("Contract: {}", state.address);
        println!("Balance: {}", state.account.balance);
        println!("Nonce: {}", state.account.nonce);
        println!("Code hash: {:?}", state.account.bytecode_hash);
        if state.hashed {
            println!(
                "Storage slots: {} (hashed-state node -- keys below are keccak256(slot), not the raw slot number)",
                state.storage.len()
            );
            for (hashed_key, value) in &state.storage {
                println!("\tkeccak256(slot)={hashed_key}: {value}");
            }
        } else {
            println!("Storage slots: {}", state.storage.len());
            for (key, value) in &state.storage {
                println!("\t{key}: {value}");
            }
        }
    }

    Ok(())
}
