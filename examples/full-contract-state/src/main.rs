//! Example demonstrating how to extract the full state of a specific contract from the reth
//! database.
//!
//! This example shows how to:
//! 1. Connect to a reth database
//! 2. Get basic account information (balance, nonce, code hash)
//! 3. Get contract bytecode
//! 4. Iterate through all storage slots for the contract

use reth_ethereum::{
    chainspec::ChainSpecBuilder,
    evm::revm::primitives::{Address, B256, U256},
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
    storage::{DBProvider, StateProvider},
};
use std::{collections::HashMap, str::FromStr};

/// Represents the complete state of a contract including account info, bytecode, and storage
#[derive(Debug, Clone)]
pub struct ContractState {
    /// The address of the contract
    pub address: Address,
    /// Basic account information (balance, nonce, code hash)
    pub account: Account,
    /// Contract bytecode (None if not a contract or doesn't exist)
    pub bytecode: Option<Bytecode>,
    /// All storage slots for the contract
    pub storage: HashMap<B256, U256>,
}

/// Extract the full state of a specific contract
pub fn extract_contract_state<P: DBProvider>(
    provider: &P,
    state_provider: &dyn StateProvider,
    contract_address: Address,
) -> ProviderResult<Option<ContractState>> {
    let account = state_provider.basic_account(&contract_address)?;
    let Some(account) = account else {
        return Ok(None);
    };

    let bytecode = state_provider.account_code(&contract_address)?;

    let mut storage_cursor = provider.tx_ref().cursor_dup_read::<tables::PlainStorageState>()?;
    let mut storage = HashMap::new();

    if let Some((_, first_entry)) = storage_cursor.seek_exact(contract_address)? {
        storage.insert(first_entry.key, first_entry.value);

        while let Some((_, entry)) = storage_cursor.next_dup()? {
            storage.insert(entry.key, entry.value);
        }
    }

    Ok(Some(ContractState { address: contract_address, account, bytecode, storage }))
}

fn main() -> eyre::Result<()> {
    let address = std::env::var("CONTRACT_ADDRESS")?;
    let contract_address = Address::from_str(&address)?;

    let datadir = std::env::var("RETH_DATADIR")?;
    let spec = ChainSpecBuilder::mainnet().build();
    let factory = EthereumNode::provider_factory_builder()
        .open_read_only(spec.into(), ReadOnlyConfig::from_datadir(datadir))?;

    let provider = factory.provider()?;
    let state_provider = factory.latest()?;
    let contract_state =
        extract_contract_state(&provider, state_provider.as_ref(), contract_address)?;

    if let Some(state) = contract_state {
        println!("Contract: {}", state.address);
        println!("Balance: {}", state.account.balance);
        println!("Nonce: {}", state.account.nonce);
        println!("Code hash: {:?}", state.account.bytecode_hash);
        println!("Storage slots: {}", state.storage.len());
        for (key, value) in &state.storage {
            println!("\t{key}: {value}");
        }
    }

    Ok(())
}
