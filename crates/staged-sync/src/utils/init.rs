use reth_db::{
    cursor::DbCursorRO,
    database::{Database, DatabaseGAT},
    mdbx::{Env, WriteMap},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{stage::StageId, Account, Bytecode, ChainSpec, H256, U256};
use reth_provider::{PostState, Transaction, TransactionError};
use std::{path::Path, sync::Arc};
use tracing::debug;

/// Opens up an existing database or creates a new one at the specified path.
pub fn init_db<P: AsRef<Path>>(path: P) -> eyre::Result<Env<WriteMap>> {
    std::fs::create_dir_all(path.as_ref())?;
    let db = Env::<WriteMap>::open(path.as_ref(), reth_db::mdbx::EnvKind::RW)?;
    db.create_tables()?;

    Ok(db)
}

/// Database initialization error type.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum InitDatabaseError {
    /// Attempted to reinitialize database with inconsistent genesis block
    #[error("Genesis hash mismatch: expected {expected}, got {actual}")]
    GenesisHashMismatch {
        /// Expected genesis hash.
        expected: H256,
        /// Actual genesis hash.
        actual: H256,
    },

    /// Higher level error encountered when using a Transaction.
    #[error(transparent)]
    TransactionError(#[from] TransactionError),

    /// Low-level database error.
    #[error(transparent)]
    DBError(#[from] reth_db::DatabaseError),
}

/// Write the genesis block if it has not already been written
#[allow(clippy::field_reassign_with_default)]
pub fn init_genesis<DB: Database>(
    db: Arc<DB>,
    chain: Arc<ChainSpec>,
) -> Result<H256, InitDatabaseError> {
    let genesis = chain.genesis();

    let header = chain.genesis_header();
    let hash = header.hash_slow();

    let tx = db.tx()?;
    if let Some((_, db_hash)) = tx.cursor_read::<tables::CanonicalHeaders>()?.first()? {
        if db_hash == hash {
            debug!("Genesis already written, skipping.");
            return Ok(hash)
        }

        return Err(InitDatabaseError::GenesisHashMismatch { expected: hash, actual: db_hash })
    }

    drop(tx);
    debug!("Writing genesis block.");
    let tx = db.tx_mut()?;

    // use transaction to insert genesis header
    let transaction = Transaction::new_raw(&db, tx);
    insert_genesis_hashes(transaction, genesis)?;

    // Insert header
    let tx = db.tx_mut()?;
    tx.put::<tables::CanonicalHeaders>(0, hash)?;
    tx.put::<tables::HeaderNumbers>(hash, 0)?;
    tx.put::<tables::BlockBodyIndices>(0, Default::default())?;
    tx.put::<tables::HeaderTD>(0, header.difficulty.into())?;
    tx.put::<tables::Headers>(0, header)?;

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
    mut transaction: Transaction<'_, DB>,
    genesis: &reth_primitives::Genesis,
) -> Result<(), InitDatabaseError> {
    // insert and hash accounts to hashing table
    let alloc_accounts =
        genesis.alloc.clone().into_iter().map(|(addr, account)| (addr, Some(account.into())));
    transaction.insert_account_for_hashing(alloc_accounts)?;

    let alloc_storage = genesis.alloc.clone().into_iter().filter_map(|(addr, account)| {
        // only return Some if there is storage
        account.storage.map(|storage| (addr, storage.into_iter().map(|(k, v)| (k, v.into()))))
    });
    transaction.insert_storage_for_hashing(alloc_storage)?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{init_genesis, InitDatabaseError};
    use reth_db::mdbx::test_utils::create_test_rw_db;
    use reth_primitives::{
        GOERLI, GOERLI_GENESIS, MAINNET, MAINNET_GENESIS, SEPOLIA, SEPOLIA_GENESIS,
    };
    use std::sync::Arc;

    #[test]
    fn success_init_genesis_mainnet() {
        let db = create_test_rw_db();
        let genesis_hash = init_genesis(db, Arc::new(MAINNET.clone())).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, MAINNET_GENESIS);
    }

    #[test]
    fn success_init_genesis_goerli() {
        let db = create_test_rw_db();
        let genesis_hash = init_genesis(db, Arc::new(GOERLI.clone())).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, GOERLI_GENESIS);
    }

    #[test]
    fn success_init_genesis_sepolia() {
        let db = create_test_rw_db();
        let genesis_hash = init_genesis(db, Arc::new(SEPOLIA.clone())).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, SEPOLIA_GENESIS);
    }

    #[test]
    fn fail_init_inconsistent_db() {
        let db = create_test_rw_db();
        init_genesis(db.clone(), Arc::new(SEPOLIA.clone())).unwrap();

        // Try to init db with a different genesis block
        let genesis_hash = init_genesis(db, Arc::new(MAINNET.clone()));

        assert_eq!(
            genesis_hash.unwrap_err(),
            InitDatabaseError::GenesisHashMismatch {
                expected: MAINNET_GENESIS,
                actual: SEPOLIA_GENESIS
            }
        )
    }
}
