use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    mdbx::{Env, WriteMap},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{keccak256, Account, Bytecode, ChainSpec, StorageEntry, H256};
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

    /// Low-level database error.
    #[error(transparent)]
    DBError(#[from] reth_db::Error),
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

    let mut account_cursor = tx.cursor_write::<tables::PlainAccountState>()?;
    let mut storage_cursor = tx.cursor_write::<tables::PlainStorageState>()?;
    let mut bytecode_cursor = tx.cursor_write::<tables::Bytecodes>()?;

    // Insert account state
    for (address, account) in &genesis.alloc {
        let mut bytecode_hash = None;
        // insert bytecode hash
        if let Some(code) = &account.code {
            let hash = keccak256(code.as_ref());
            bytecode_cursor.upsert(hash, Bytecode::new_raw_with_hash(code.0.clone(), hash))?;
            bytecode_hash = Some(hash);
        }
        // insert plain account.
        account_cursor.upsert(
            *address,
            Account {
                nonce: account.nonce.unwrap_or_default(),
                balance: account.balance,
                bytecode_hash,
            },
        )?;
        // insert plain storages
        if let Some(storage) = &account.storage {
            for (&key, &value) in storage {
                storage_cursor.upsert(*address, StorageEntry { key, value: value.into() })?
            }
        }
    }
    // Drop all cursor so we can commit the changes at the end of the fn.
    drop(account_cursor);
    drop(storage_cursor);
    drop(bytecode_cursor);

    // Insert header
    tx.put::<tables::CanonicalHeaders>(0, hash)?;
    tx.put::<tables::HeaderNumbers>(hash, 0)?;
    tx.put::<tables::BlockBodies>(0, Default::default())?;
    tx.put::<tables::BlockTransitionIndex>(0, 0)?;
    tx.put::<tables::HeaderTD>(0, header.difficulty.into())?;
    tx.put::<tables::Headers>(0, header)?;

    tx.commit()?;
    Ok(hash)
}

#[cfg(test)]
mod tests {

    use std::sync::Arc;

    use super::{init_genesis, InitDatabaseError};
    use reth_db::mdbx::test_utils::create_test_rw_db;
    use reth_primitives::{
        GOERLI, GOERLI_GENESIS, MAINNET, MAINNET_GENESIS, SEPOLIA, SEPOLIA_GENESIS,
    };

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
