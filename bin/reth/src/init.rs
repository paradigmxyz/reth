//! Reth genesis initialization utility functions.
use reth_db::{
    cursor::DbCursorRO,
    database::{Database, DatabaseGAT},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{stage::StageId, Account, Bytecode, ChainSpec, StorageEntry, H256, U256};
use reth_provider::{DatabaseProviderRW, HashingWriter, HistoryWriter, PostState, ProviderFactory};
use std::{collections::BTreeMap, sync::Arc};
use tracing::debug;

/// Database initialization error type.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum InitDatabaseError {
    /// An existing genesis block was found in the database, and its hash did not match the hash of
    /// the chainspec.
    #[error("Genesis hash in the database does not match the specified chainspec: chainspec is {chainspec_hash}, database is {database_hash}")]
    GenesisHashMismatch {
        /// Expected genesis hash.
        chainspec_hash: H256,
        /// Actual genesis hash.
        database_hash: H256,
    },

    /// Low-level database error.
    #[error(transparent)]
    DBError(#[from] reth_db::DatabaseError),

    /// Internal error.
    #[error(transparent)]
    InternalError(#[from] reth_interfaces::Error),
}

/// Write the genesis block if it has not already been written
#[allow(clippy::field_reassign_with_default)]
pub fn init_genesis<DB: Database>(
    db: Arc<DB>,
    chain: Arc<ChainSpec>,
) -> Result<H256, InitDatabaseError> {
    let genesis = chain.genesis();

    let hash = chain.genesis_hash();

    let tx = db.tx()?;
    if let Some((_, db_hash)) = tx.cursor_read::<tables::CanonicalHeaders>()?.first()? {
        if db_hash == hash {
            debug!("Genesis already written, skipping.");
            return Ok(hash)
        }

        return Err(InitDatabaseError::GenesisHashMismatch {
            chainspec_hash: hash,
            database_hash: db_hash,
        })
    }

    drop(tx);
    debug!("Writing genesis block.");

    // use transaction to insert genesis header
    let factory = ProviderFactory::new(&db, chain.clone());
    let provider_rw = factory.provider_rw()?;
    insert_genesis_hashes(&provider_rw, genesis)?;
    insert_genesis_history(&provider_rw, genesis)?;
    provider_rw.commit()?;

    // Insert header
    let tx = db.tx_mut()?;
    insert_genesis_header::<DB>(&tx, chain.clone())?;

    insert_genesis_state::<DB>(&tx, genesis)?;

    // insert sync stage
    for stage in StageId::ALL.iter() {
        tx.put::<tables::SyncStage>(stage.to_string(), Default::default())?;
    }

    tx.commit()?;
    Ok(hash)
}

/// Inserts the genesis state into the database.
pub fn insert_genesis_state<DB: Database>(
    tx: &<DB as DatabaseGAT<'_>>::TXMut,
    genesis: &reth_primitives::Genesis,
) -> Result<(), InitDatabaseError> {
    let mut state = PostState::default();

    for (address, account) in &genesis.alloc {
        let mut bytecode_hash = None;
        if let Some(code) = &account.code {
            let bytecode = Bytecode::new_raw(code.0.clone());
            // FIXME: Can bytecode_hash be Some(Bytes::new()) here?
            bytecode_hash = Some(bytecode.hash);
            state.add_bytecode(bytecode.hash, bytecode);
        }
        state.create_account(
            0,
            *address,
            Account { nonce: account.nonce.unwrap_or(0), balance: account.balance, bytecode_hash },
        );
        if let Some(storage) = &account.storage {
            let mut storage_changes = reth_provider::post_state::StorageChangeset::new();
            for (&key, &value) in storage {
                storage_changes
                    .insert(U256::from_be_bytes(key.0), (U256::ZERO, U256::from_be_bytes(value.0)));
            }
            state.change_storage(0, *address, storage_changes);
        }
    }
    state.write_to_db(tx, 0)?;

    Ok(())
}

/// Inserts hashes for the genesis state.
pub fn insert_genesis_hashes<DB: Database>(
    provider: &DatabaseProviderRW<'_, &DB>,
    genesis: &reth_primitives::Genesis,
) -> Result<(), InitDatabaseError> {
    // insert and hash accounts to hashing table
    let alloc_accounts =
        genesis.alloc.clone().into_iter().map(|(addr, account)| (addr, Some(account.into())));
    provider.insert_account_for_hashing(alloc_accounts)?;

    let alloc_storage = genesis.alloc.clone().into_iter().filter_map(|(addr, account)| {
        // only return Some if there is storage
        account.storage.map(|storage| {
            (
                addr,
                storage.into_iter().map(|(key, value)| StorageEntry { key, value: value.into() }),
            )
        })
    });
    provider.insert_storage_for_hashing(alloc_storage)?;

    Ok(())
}

/// Inserts history indices for genesis accounts and storage.
pub fn insert_genesis_history<DB: Database>(
    provider: &DatabaseProviderRW<'_, &DB>,
    genesis: &reth_primitives::Genesis,
) -> Result<(), InitDatabaseError> {
    let account_transitions =
        genesis.alloc.keys().map(|addr| (*addr, vec![0])).collect::<BTreeMap<_, _>>();
    provider.insert_account_history_index(account_transitions)?;

    let storage_transitions = genesis
        .alloc
        .iter()
        .filter_map(|(addr, account)| account.storage.as_ref().map(|storage| (addr, storage)))
        .flat_map(|(addr, storage)| storage.iter().map(|(key, _)| ((*addr, *key), vec![0])))
        .collect::<BTreeMap<_, _>>();
    provider.insert_storage_history_index(storage_transitions)?;

    Ok(())
}

/// Inserts header for the genesis state.
pub fn insert_genesis_header<DB: Database>(
    tx: &<DB as DatabaseGAT<'_>>::TXMut,
    chain: Arc<ChainSpec>,
) -> Result<(), InitDatabaseError> {
    let header = chain.sealed_genesis_header();

    tx.put::<tables::CanonicalHeaders>(0, header.hash)?;
    tx.put::<tables::HeaderNumbers>(header.hash, 0)?;
    tx.put::<tables::BlockBodyIndices>(0, Default::default())?;
    tx.put::<tables::HeaderTD>(0, header.difficulty.into())?;
    tx.put::<tables::Headers>(0, header.header)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use reth_db::{
        models::{storage_sharded_key::StorageShardedKey, ShardedKey},
        table::{Table, TableRow},
        test_utils::create_test_rw_db,
        DatabaseEnv,
    };
    use reth_primitives::{
        Address, Chain, ForkTimestamps, Genesis, GenesisAccount, IntegerList, GOERLI,
        GOERLI_GENESIS, MAINNET, MAINNET_GENESIS, SEPOLIA, SEPOLIA_GENESIS,
    };
    use std::collections::HashMap;

    #[allow(clippy::type_complexity)]
    fn collect_table_entries<DB, T>(
        tx: &<DB as DatabaseGAT<'_>>::TX,
    ) -> Result<Vec<TableRow<T>>, InitDatabaseError>
    where
        DB: Database,
        T: Table,
    {
        Ok(tx.cursor_read::<T>()?.walk_range(..)?.collect::<Result<Vec<_>, _>>()?)
    }

    #[test]
    fn success_init_genesis_mainnet() {
        let db = create_test_rw_db();
        let genesis_hash = init_genesis(db, MAINNET.clone()).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, MAINNET_GENESIS);
    }

    #[test]
    fn success_init_genesis_goerli() {
        let db = create_test_rw_db();
        let genesis_hash = init_genesis(db, GOERLI.clone()).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, GOERLI_GENESIS);
    }

    #[test]
    fn success_init_genesis_sepolia() {
        let db = create_test_rw_db();
        let genesis_hash = init_genesis(db, SEPOLIA.clone()).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, SEPOLIA_GENESIS);
    }

    #[test]
    fn fail_init_inconsistent_db() {
        let db = create_test_rw_db();
        init_genesis(db.clone(), SEPOLIA.clone()).unwrap();

        // Try to init db with a different genesis block
        let genesis_hash = init_genesis(db, MAINNET.clone());

        assert_eq!(
            genesis_hash.unwrap_err(),
            InitDatabaseError::GenesisHashMismatch {
                chainspec_hash: MAINNET_GENESIS,
                database_hash: SEPOLIA_GENESIS
            }
        )
    }

    #[test]
    fn init_genesis_history() {
        let address_with_balance = Address::from_low_u64_be(1);
        let address_with_storage = Address::from_low_u64_be(2);
        let storage_key = H256::from_low_u64_be(1);
        let chain_spec = Arc::new(ChainSpec {
            chain: Chain::Id(1),
            genesis: Genesis {
                alloc: HashMap::from([
                    (
                        address_with_balance,
                        GenesisAccount { balance: U256::from(1), ..Default::default() },
                    ),
                    (
                        address_with_storage,
                        GenesisAccount {
                            storage: Some(HashMap::from([(storage_key, H256::random())])),
                            ..Default::default()
                        },
                    ),
                ]),
                ..Default::default()
            },
            hardforks: BTreeMap::default(),
            fork_timestamps: ForkTimestamps::default(),
            genesis_hash: None,
            paris_block_and_final_difficulty: None,
            deposit_contract: None,
            ..Default::default()
        });

        let db = create_test_rw_db();
        init_genesis(db.clone(), chain_spec).unwrap();

        let tx = db.tx().expect("failed to init tx");

        assert_eq!(
            collect_table_entries::<Arc<DatabaseEnv>, tables::AccountHistory>(&tx)
                .expect("failed to collect"),
            vec![
                (ShardedKey::new(address_with_balance, u64::MAX), IntegerList::new([0]).unwrap()),
                (ShardedKey::new(address_with_storage, u64::MAX), IntegerList::new([0]).unwrap())
            ],
        );

        assert_eq!(
            collect_table_entries::<Arc<DatabaseEnv>, tables::StorageHistory>(&tx)
                .expect("failed to collect"),
            vec![(
                StorageShardedKey::new(address_with_storage, storage_key, u64::MAX),
                IntegerList::new([0]).unwrap()
            )],
        );
    }
}
