use eyre::WrapErr;
use reth_db::{
    cursor::DbCursorRO,
    database::{Database, DatabaseGAT},
    is_database_empty,
    mdbx::{Env, WriteMap},
    tables,
    transaction::{DbTx, DbTxMut},
    version::{check_db_version_file, create_db_version_file, DatabaseVersionError},
};
use reth_primitives::{stage::StageId, Account, Bytecode, ChainSpec, H256, U256};
use reth_provider::{
    AccountWriter, DatabaseProviderRW, PostState, ProviderFactory, TransactionError,
};
use std::{fs, path::Path, sync::Arc};
use tracing::debug;

/// Opens up an existing database or creates a new one at the specified path.
pub fn init_db<P: AsRef<Path>>(path: P) -> eyre::Result<Env<WriteMap>> {
    if is_database_empty(&path) {
        fs::create_dir_all(&path).wrap_err_with(|| {
            format!("Could not create database directory {}", path.as_ref().display())
        })?;
        create_db_version_file(&path)?;
    } else {
        match check_db_version_file(&path) {
            Ok(_) => (),
            Err(DatabaseVersionError::MissingFile) => create_db_version_file(&path)?,
            Err(err) => return Err(err.into()),
        }
    }

    let db = Env::<WriteMap>::open(path.as_ref(), reth_db::mdbx::EnvKind::RW)?;

    db.create_tables()?;

    Ok(db)
}

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

    /// Higher level error encountered when using a Transaction.
    #[error(transparent)]
    TransactionError(#[from] TransactionError),

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
    insert_genesis_hashes(provider_rw, genesis)?;

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
    state.write_to_db(tx)?;

    Ok(())
}

/// Inserts hashes for the genesis state.
pub fn insert_genesis_hashes<DB: Database>(
    provider: DatabaseProviderRW<'_, &DB>,
    genesis: &reth_primitives::Genesis,
) -> Result<(), InitDatabaseError> {
    // insert and hash accounts to hashing table
    let alloc_accounts =
        genesis.alloc.clone().into_iter().map(|(addr, account)| (addr, Some(account.into())));
    provider.insert_account_for_hashing(alloc_accounts)?;

    let alloc_storage = genesis.alloc.clone().into_iter().filter_map(|(addr, account)| {
        // only return Some if there is storage
        account.storage.map(|storage| (addr, storage.into_iter().map(|(k, v)| (k, v.into()))))
    });
    provider.insert_storage_for_hashing(alloc_storage)?;
    provider.commit()?;

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
    use super::{init_db, init_genesis, InitDatabaseError};
    use assert_matches::assert_matches;
    use reth_db::{
        mdbx::test_utils::create_test_rw_db,
        version::{db_version_file_path, DatabaseVersionError},
    };
    use reth_primitives::{
        GOERLI, GOERLI_GENESIS, MAINNET, MAINNET_GENESIS, SEPOLIA, SEPOLIA_GENESIS,
    };
    use std::fs;
    use tempfile::tempdir;

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
    fn db_version() {
        let path = tempdir().unwrap();

        // Database is empty
        {
            let db = init_db(&path);
            assert_matches!(db, Ok(_));
        }

        // Database is not empty, current version is the same as in the file
        {
            let db = init_db(&path);
            assert_matches!(db, Ok(_));
        }

        // Database is not empty, version file is malformed
        {
            fs::write(path.path().join(db_version_file_path(&path)), "invalid-version").unwrap();
            let db = init_db(&path);
            assert!(db.is_err());
            assert_matches!(
                db.unwrap_err().downcast_ref::<DatabaseVersionError>(),
                Some(DatabaseVersionError::MalformedFile)
            )
        }

        // Database is not empty, version file contains not matching version
        {
            fs::write(path.path().join(db_version_file_path(&path)), "0").unwrap();
            let db = init_db(&path);
            assert!(db.is_err());
            assert_matches!(
                db.unwrap_err().downcast_ref::<DatabaseVersionError>(),
                Some(DatabaseVersionError::VersionMismatch { version: 0 })
            )
        }
    }
}
