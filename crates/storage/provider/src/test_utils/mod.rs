use crate::{providers::StaticFileProvider, HashingWriter, ProviderFactory, TrieWriter};
use reth_chainspec::{ChainSpec, MAINNET};
use reth_db::{
    test_utils::{create_test_rw_db, create_test_static_files_dir, TempDatabase},
    Database, DatabaseEnv,
};
use reth_errors::ProviderResult;
use reth_primitives::{Account, StorageEntry, B256};
use reth_trie::StateRoot;
use reth_trie_db::DatabaseStateRoot;
use std::sync::Arc;

pub mod blocks;
mod mock;
mod noop;

pub use mock::{ExtendedAccount, MockEthProvider};
pub use noop::NoopProvider;
pub use reth_chain_state::test_utils::TestCanonStateSubscriptions;

/// Creates test provider factory with mainnet chain spec.
pub fn create_test_provider_factory() -> ProviderFactory<Arc<TempDatabase<DatabaseEnv>>> {
    create_test_provider_factory_with_chain_spec(MAINNET.clone())
}

/// Creates test provider factory with provided chain spec.
pub fn create_test_provider_factory_with_chain_spec(
    chain_spec: Arc<ChainSpec>,
) -> ProviderFactory<Arc<TempDatabase<DatabaseEnv>>> {
    let (static_dir, _) = create_test_static_files_dir();
    let db = create_test_rw_db();
    ProviderFactory::new(
        db,
        chain_spec,
        StaticFileProvider::read_write(static_dir.into_path()).expect("static file provider"),
    )
}

/// Inserts the genesis alloc from the provided chain spec into the trie.
pub fn insert_genesis<DB: Database>(
    provider_factory: &ProviderFactory<DB>,
    chain_spec: Arc<ChainSpec>,
) -> ProviderResult<B256> {
    let provider = provider_factory.provider_rw()?;

    // Hash accounts and insert them into hashing table.
    let genesis = chain_spec.genesis();
    let alloc_accounts = genesis
        .alloc
        .iter()
        .map(|(addr, account)| (*addr, Some(Account::from_genesis_account(account))));
    provider.insert_account_for_hashing(alloc_accounts).unwrap();

    let alloc_storage = genesis.alloc.clone().into_iter().filter_map(|(addr, account)| {
        // Only return `Some` if there is storage.
        account.storage.map(|storage| {
            (
                addr,
                storage.into_iter().map(|(key, value)| StorageEntry { key, value: value.into() }),
            )
        })
    });
    provider.insert_storage_for_hashing(alloc_storage)?;

    let (root, updates) = StateRoot::from_tx(provider.tx_ref())
        .root_with_updates()
        .map_err(Into::<reth_db::DatabaseError>::into)?;
    provider.write_trie_updates(&updates).unwrap();

    provider.commit()?;

    Ok(root)
}
