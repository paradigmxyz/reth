//! Module that interacts with MDBX.

use crate::{
    database::{Database, DatabaseGAT},
    tables::{TableType, Tables},
    utils::default_page_size,
    DatabaseError,
};
use reth_interfaces::db::LogLevel;
use reth_libmdbx::{
    DatabaseFlags, Environment, EnvironmentFlags, EnvironmentKind, Geometry, Mode, PageSize,
    SyncMode, RO, RW,
};
use std::{ops::Deref, path::Path};
use tx::Tx;

pub mod cursor;
pub mod tx;

const GIGABYTE: usize = 1024 * 1024 * 1024;
const TERABYTE: usize = GIGABYTE * 1024;

/// MDBX allows up to 32767 readers (`MDBX_READERS_LIMIT`), but we limit it to slightly below that
const DEFAULT_MAX_READERS: u64 = 32_000;

/// Environment used when opening a MDBX environment. RO/RW.
#[derive(Debug)]
pub enum EnvKind {
    /// Read-only MDBX environment.
    RO,
    /// Read-write MDBX environment.
    RW,
}

/// Wrapper for the libmdbx environment.
#[derive(Debug)]
pub struct Env<E: EnvironmentKind> {
    /// Libmdbx-sys environment.
    pub inner: Environment<E>,
}

impl<'a, E: EnvironmentKind> DatabaseGAT<'a> for Env<E> {
    type TX = tx::Tx<'a, RO, E>;
    type TXMut = tx::Tx<'a, RW, E>;
}

impl<E: EnvironmentKind> Database for Env<E> {
    fn tx(&self) -> Result<<Self as DatabaseGAT<'_>>::TX, DatabaseError> {
        Ok(Tx::new(
            self.inner.begin_ro_txn().map_err(|e| DatabaseError::InitTransaction(e.into()))?,
        ))
    }

    fn tx_mut(&self) -> Result<<Self as DatabaseGAT<'_>>::TXMut, DatabaseError> {
        Ok(Tx::new(
            self.inner.begin_rw_txn().map_err(|e| DatabaseError::InitTransaction(e.into()))?,
        ))
    }
}

impl<E: EnvironmentKind> Env<E> {
    /// Opens the database at the specified path with the given `EnvKind`.
    ///
    /// It does not create the tables, for that call [`Env::create_tables`].
    pub fn open(
        path: &Path,
        kind: EnvKind,
        log_level: Option<LogLevel>,
    ) -> Result<Env<E>, DatabaseError> {
        let mode = match kind {
            EnvKind::RO => Mode::ReadOnly,
            EnvKind::RW => Mode::ReadWrite { sync_mode: SyncMode::Durable },
        };

        let mut inner_env = Environment::new();
        inner_env.set_max_dbs(Tables::ALL.len());
        inner_env.set_geometry(Geometry {
            // Maximum database size of 4 terabytes
            size: Some(0..(4 * TERABYTE)),
            // We grow the database in increments of 4 gigabytes
            growth_step: Some(4 * GIGABYTE as isize),
            // The database never shrinks
            shrink_threshold: None,
            page_size: Some(PageSize::Set(default_page_size())),
        });
        inner_env.set_flags(EnvironmentFlags {
            mode,
            // We disable readahead because it improves performance for linear scans, but
            // worsens it for random access (which is our access pattern outside of sync)
            no_rdahead: true,
            coalesce: true,
            ..Default::default()
        });
        // configure more readers
        inner_env.set_max_readers(DEFAULT_MAX_READERS);

        if let Some(log_level) = log_level {
            // Levels higher than [LogLevel::Notice] require libmdbx built with `MDBX_DEBUG` option.
            let is_log_level_available = if cfg!(debug_assertions) {
                true
            } else {
                matches!(
                    log_level,
                    LogLevel::Fatal | LogLevel::Error | LogLevel::Warn | LogLevel::Notice
                )
            };
            if is_log_level_available {
                inner_env.set_log_level(match log_level {
                    LogLevel::Fatal => 0,
                    LogLevel::Error => 1,
                    LogLevel::Warn => 2,
                    LogLevel::Notice => 3,
                    LogLevel::Verbose => 4,
                    LogLevel::Debug => 5,
                    LogLevel::Trace => 6,
                    LogLevel::Extra => 7,
                });
            } else {
                return Err(DatabaseError::LogLevelUnavailable(log_level))
            }
        }

        let env =
            Env { inner: inner_env.open(path).map_err(|e| DatabaseError::FailedToOpen(e.into()))? };

        Ok(env)
    }

    /// Creates all the defined tables, if necessary.
    pub fn create_tables(&self) -> Result<(), DatabaseError> {
        let tx = self.inner.begin_rw_txn().map_err(|e| DatabaseError::InitTransaction(e.into()))?;

        for table in Tables::ALL {
            let flags = match table.table_type() {
                TableType::Table => DatabaseFlags::default(),
                TableType::DupSort => DatabaseFlags::DUP_SORT,
            };

            tx.create_db(Some(table.name()), flags)
                .map_err(|e| DatabaseError::TableCreation(e.into()))?;
        }

        tx.commit().map_err(|e| DatabaseError::Commit(e.into()))?;

        Ok(())
    }
}

impl<E: EnvironmentKind> Deref for Env<E> {
    type Target = Environment<E>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, ReverseWalker, Walker},
        database::Database,
        models::{AccountBeforeTx, ShardedKey},
        tables::{AccountHistory, CanonicalHeaders, Headers, PlainAccountState, PlainStorageState},
        test_utils::*,
        transaction::{DbTx, DbTxMut},
        AccountChangeSet, DatabaseError,
    };
    use reth_libmdbx::{NoWriteMap, WriteMap};
    use reth_primitives::{Account, Address, Header, IntegerList, StorageEntry, H160, H256, U256};
    use std::{path::Path, str::FromStr, sync::Arc};
    use tempfile::TempDir;

    /// Create database for testing
    fn create_test_db<E: EnvironmentKind>(kind: EnvKind) -> Arc<Env<E>> {
        Arc::new(create_test_db_with_path(
            kind,
            &tempfile::TempDir::new().expect(ERROR_TEMPDIR).into_path(),
        ))
    }

    /// Create database for testing with specified path
    fn create_test_db_with_path<E: EnvironmentKind>(kind: EnvKind, path: &Path) -> Env<E> {
        let env = Env::<E>::open(path, kind, None).expect(ERROR_DB_CREATION);
        env.create_tables().expect(ERROR_TABLE_CREATION);
        env
    }

    const ERROR_DB_CREATION: &str = "Not able to create the mdbx file.";
    const ERROR_PUT: &str = "Not able to insert value into table.";
    const ERROR_APPEND: &str = "Not able to append the value to the table.";
    const ERROR_UPSERT: &str = "Not able to upsert the value to the table.";
    const ERROR_GET: &str = "Not able to get value from table.";
    const ERROR_COMMIT: &str = "Not able to commit transaction.";
    const ERROR_RETURN_VALUE: &str = "Mismatching result.";
    const ERROR_INIT_TX: &str = "Failed to create a MDBX transaction.";
    const ERROR_ETH_ADDRESS: &str = "Invalid address.";

    #[test]
    fn db_creation() {
        create_test_db::<NoWriteMap>(EnvKind::RW);
    }

    #[test]
    fn db_manual_put_get() {
        let env = create_test_db::<NoWriteMap>(EnvKind::RW);

        let value = Header::default();
        let key = 1u64;

        // PUT
        let tx = env.tx_mut().expect(ERROR_INIT_TX);
        tx.put::<Headers>(key, value.clone()).expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        // GET
        let tx = env.tx().expect(ERROR_INIT_TX);
        let result = tx.get::<Headers>(key).expect(ERROR_GET);
        assert!(result.expect(ERROR_RETURN_VALUE) == value);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_cursor_walk() {
        let env = create_test_db::<NoWriteMap>(EnvKind::RW);

        let value = Header::default();
        let key = 1u64;

        // PUT
        let tx = env.tx_mut().expect(ERROR_INIT_TX);
        tx.put::<Headers>(key, value.clone()).expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        // Cursor
        let tx = env.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<Headers>().unwrap();

        let first = cursor.first().unwrap();
        assert!(first.is_some(), "First should be our put");

        // Walk
        let walk = cursor.walk(Some(key)).unwrap();
        let first = walk.into_iter().next().unwrap().unwrap();
        assert_eq!(first.1, value, "First next should be put value");
    }

    #[test]
    fn db_cursor_walk_range() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);

        // PUT (0, 0), (1, 0), (2, 0), (3, 0)
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 2, 3]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, H256::zero()))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();

        // [1, 3)
        let mut walker = cursor.walk_range(1..3).unwrap();
        assert_eq!(walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((2, H256::zero()))));
        assert_eq!(walker.next(), None);
        // next() returns None after walker is done
        assert_eq!(walker.next(), None);

        // [1, 2]
        let mut walker = cursor.walk_range(1..=2).unwrap();
        assert_eq!(walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((2, H256::zero()))));
        // next() returns None after walker is done
        assert_eq!(walker.next(), None);

        // [1, ∞)
        let mut walker = cursor.walk_range(1..).unwrap();
        assert_eq!(walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((2, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((3, H256::zero()))));
        // next() returns None after walker is done
        assert_eq!(walker.next(), None);

        // [2, 4)
        let mut walker = cursor.walk_range(2..4).unwrap();
        assert_eq!(walker.next(), Some(Ok((2, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((3, H256::zero()))));
        assert_eq!(walker.next(), None);
        // next() returns None after walker is done
        assert_eq!(walker.next(), None);

        // (∞, 3)
        let mut walker = cursor.walk_range(..3).unwrap();
        assert_eq!(walker.next(), Some(Ok((0, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((2, H256::zero()))));
        // next() returns None after walker is done
        assert_eq!(walker.next(), None);

        // (∞, ∞)
        let mut walker = cursor.walk_range(..).unwrap();
        assert_eq!(walker.next(), Some(Ok((0, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((2, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((3, H256::zero()))));
        // next() returns None after walker is done
        assert_eq!(walker.next(), None);
    }

    #[test]
    fn db_cursor_walk_range_on_dup_table() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);

        let address0 = Address::zero();
        let address1 = Address::from_low_u64_be(1);
        let address2 = Address::from_low_u64_be(2);

        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        tx.put::<AccountChangeSet>(0, AccountBeforeTx { address: address0, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSet>(0, AccountBeforeTx { address: address1, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSet>(0, AccountBeforeTx { address: address2, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSet>(1, AccountBeforeTx { address: address0, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSet>(1, AccountBeforeTx { address: address1, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSet>(1, AccountBeforeTx { address: address2, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSet>(2, AccountBeforeTx { address: address0, info: None }) // <- should not be returned by the walker
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<AccountChangeSet>().unwrap();

        let entries = cursor.walk_range(..).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(entries.len(), 7);

        let mut walker = cursor.walk_range(0..=1).unwrap();
        assert_eq!(walker.next(), Some(Ok((0, AccountBeforeTx { address: address0, info: None }))));
        assert_eq!(walker.next(), Some(Ok((0, AccountBeforeTx { address: address1, info: None }))));
        assert_eq!(walker.next(), Some(Ok((0, AccountBeforeTx { address: address2, info: None }))));
        assert_eq!(walker.next(), Some(Ok((1, AccountBeforeTx { address: address0, info: None }))));
        assert_eq!(walker.next(), Some(Ok((1, AccountBeforeTx { address: address1, info: None }))));
        assert_eq!(walker.next(), Some(Ok((1, AccountBeforeTx { address: address2, info: None }))));
        assert_eq!(walker.next(), None);
    }

    #[allow(clippy::reversed_empty_ranges)]
    #[test]
    fn db_cursor_walk_range_invalid() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);

        // PUT (0, 0), (1, 0), (2, 0), (3, 0)
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 2, 3]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, H256::zero()))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();

        // start bound greater than end bound
        let mut res = cursor.walk_range(3..1).unwrap();
        assert_eq!(res.next(), None);

        // start bound greater than end bound
        let mut res = cursor.walk_range(15..=2).unwrap();
        assert_eq!(res.next(), None);

        // returning nothing
        let mut walker = cursor.walk_range(1..1).unwrap();
        assert_eq!(walker.next(), None);
    }

    #[test]
    fn db_walker() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);

        // PUT (0, 0), (1, 0), (3, 0)
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 3]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, H256::zero()))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();

        let mut walker = Walker::new(&mut cursor, None);

        assert_eq!(walker.next(), Some(Ok((0, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((3, H256::zero()))));
        assert_eq!(walker.next(), None);

        // transform to ReverseWalker
        let mut reverse_walker = walker.rev();
        assert_eq!(reverse_walker.next(), Some(Ok((3, H256::zero()))));
        assert_eq!(reverse_walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(reverse_walker.next(), Some(Ok((0, H256::zero()))));
        assert_eq!(reverse_walker.next(), None);
    }

    #[test]
    fn db_reverse_walker() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);

        // PUT (0, 0), (1, 0), (3, 0)
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 3]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, H256::zero()))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();

        let mut reverse_walker = ReverseWalker::new(&mut cursor, None);

        assert_eq!(reverse_walker.next(), Some(Ok((3, H256::zero()))));
        assert_eq!(reverse_walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(reverse_walker.next(), Some(Ok((0, H256::zero()))));
        assert_eq!(reverse_walker.next(), None);

        // transform to Walker
        let mut walker = reverse_walker.forward();
        assert_eq!(walker.next(), Some(Ok((0, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(walker.next(), Some(Ok((3, H256::zero()))));
        assert_eq!(walker.next(), None);
    }

    #[test]
    fn db_walk_back() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);

        // PUT (0, 0), (1, 0), (3, 0)
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 3]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, H256::zero()))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();

        let mut reverse_walker = cursor.walk_back(Some(1)).unwrap();
        assert_eq!(reverse_walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(reverse_walker.next(), Some(Ok((0, H256::zero()))));
        assert_eq!(reverse_walker.next(), None);

        let mut reverse_walker = cursor.walk_back(Some(2)).unwrap();
        assert_eq!(reverse_walker.next(), Some(Ok((3, H256::zero()))));
        assert_eq!(reverse_walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(reverse_walker.next(), Some(Ok((0, H256::zero()))));
        assert_eq!(reverse_walker.next(), None);

        let mut reverse_walker = cursor.walk_back(Some(4)).unwrap();
        assert_eq!(reverse_walker.next(), Some(Ok((3, H256::zero()))));
        assert_eq!(reverse_walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(reverse_walker.next(), Some(Ok((0, H256::zero()))));
        assert_eq!(reverse_walker.next(), None);

        let mut reverse_walker = cursor.walk_back(None).unwrap();
        assert_eq!(reverse_walker.next(), Some(Ok((3, H256::zero()))));
        assert_eq!(reverse_walker.next(), Some(Ok((1, H256::zero()))));
        assert_eq!(reverse_walker.next(), Some(Ok((0, H256::zero()))));
        assert_eq!(reverse_walker.next(), None);
    }

    #[test]
    fn db_cursor_seek_exact_or_previous_key() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);

        // PUT
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 3]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, H256::zero()))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        // Cursor
        let missing_key = 2;
        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        assert_eq!(cursor.current(), Ok(None));

        // Seek exact
        let exact = cursor.seek_exact(missing_key).unwrap();
        assert_eq!(exact, None);
        assert_eq!(cursor.current(), Ok(Some((missing_key + 1, H256::zero()))));
        assert_eq!(cursor.prev(), Ok(Some((missing_key - 1, H256::zero()))));
        assert_eq!(cursor.prev(), Ok(Some((missing_key - 2, H256::zero()))));
    }

    #[test]
    fn db_cursor_insert() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);

        // PUT
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 3, 4, 5]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, H256::zero()))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let key_to_insert = 2;
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();

        // INSERT
        assert_eq!(cursor.insert(key_to_insert, H256::zero()), Ok(()));
        assert_eq!(cursor.current(), Ok(Some((key_to_insert, H256::zero()))));

        // INSERT (failure)
        assert_eq!(cursor.insert(key_to_insert, H256::zero()), Err(DatabaseError::Write(-30799)));
        assert_eq!(cursor.current(), Ok(Some((key_to_insert, H256::zero()))));

        tx.commit().expect(ERROR_COMMIT);

        // Confirm the result
        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let res = cursor.walk(None).unwrap().map(|res| res.unwrap().0).collect::<Vec<_>>();
        assert_eq!(res, vec![0, 1, 2, 3, 4, 5]);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_cursor_insert_dup() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        let mut dup_cursor = tx.cursor_dup_write::<PlainStorageState>().unwrap();
        let key = Address::random();
        let subkey1 = H256::random();
        let subkey2 = H256::random();

        let entry1 = StorageEntry { key: subkey1, value: U256::ZERO };
        assert!(dup_cursor.insert(key, entry1).is_ok());

        // Can't insert
        let entry2 = StorageEntry { key: subkey2, value: U256::ZERO };
        assert!(dup_cursor.insert(key, entry2).is_err());
    }

    #[test]
    fn db_cursor_delete_current_non_existent() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        let key1 = Address::from_low_u64_be(1);
        let key2 = Address::from_low_u64_be(2);
        let key3 = Address::from_low_u64_be(3);
        let mut cursor = tx.cursor_write::<PlainAccountState>().unwrap();

        assert!(cursor.insert(key1, Account::default()).is_ok());
        assert!(cursor.insert(key2, Account::default()).is_ok());
        assert!(cursor.insert(key3, Account::default()).is_ok());

        // Seek & delete key2
        cursor.seek_exact(key2).unwrap();
        assert_eq!(cursor.delete_current(), Ok(()));
        assert_eq!(cursor.seek_exact(key2), Ok(None));

        // Seek & delete key2 again
        assert_eq!(cursor.seek_exact(key2), Ok(None));
        assert_eq!(cursor.delete_current(), Ok(()));
        // Assert that key1 is still there
        assert_eq!(cursor.seek_exact(key1), Ok(Some((key1, Account::default()))));
        // Assert that key3 was deleted
        assert_eq!(cursor.seek_exact(key3), Ok(None));
    }

    #[test]
    fn db_cursor_insert_wherever_cursor_is() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        // PUT
        vec![0, 1, 3, 5, 7, 9]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, H256::zero()))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();

        // INSERT (cursor starts at last)
        cursor.last().unwrap();
        assert_eq!(cursor.current(), Ok(Some((9, H256::zero()))));

        for pos in (2..=8).step_by(2) {
            assert_eq!(cursor.insert(pos, H256::zero()), Ok(()));
            assert_eq!(cursor.current(), Ok(Some((pos, H256::zero()))));
        }
        tx.commit().expect(ERROR_COMMIT);

        // Confirm the result
        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let res = cursor.walk(None).unwrap().map(|res| res.unwrap().0).collect::<Vec<_>>();
        assert_eq!(res, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_cursor_append() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);

        // PUT
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 2, 3, 4]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, H256::zero()))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        // APPEND
        let key_to_append = 5;
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();
        assert_eq!(cursor.append(key_to_append, H256::zero()), Ok(()));
        tx.commit().expect(ERROR_COMMIT);

        // Confirm the result
        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let res = cursor.walk(None).unwrap().map(|res| res.unwrap().0).collect::<Vec<_>>();
        assert_eq!(res, vec![0, 1, 2, 3, 4, 5]);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_cursor_append_failure() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);

        // PUT
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 3, 4, 5]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, H256::zero()))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        // APPEND
        let key_to_append = 2;
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();
        assert_eq!(cursor.append(key_to_append, H256::zero()), Err(DatabaseError::Write(-30418)));
        assert_eq!(cursor.current(), Ok(Some((5, H256::zero())))); // the end of table
        tx.commit().expect(ERROR_COMMIT);

        // Confirm the result
        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let res = cursor.walk(None).unwrap().map(|res| res.unwrap().0).collect::<Vec<_>>();
        assert_eq!(res, vec![0, 1, 3, 4, 5]);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_cursor_upsert() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        let mut cursor = tx.cursor_write::<PlainAccountState>().unwrap();
        let key = Address::random();

        let account = Account::default();
        cursor.upsert(key, account).expect(ERROR_UPSERT);
        assert_eq!(cursor.seek_exact(key), Ok(Some((key, account))));

        let account = Account { nonce: 1, ..Default::default() };
        cursor.upsert(key, account).expect(ERROR_UPSERT);
        assert_eq!(cursor.seek_exact(key), Ok(Some((key, account))));

        let account = Account { nonce: 2, ..Default::default() };
        cursor.upsert(key, account).expect(ERROR_UPSERT);
        assert_eq!(cursor.seek_exact(key), Ok(Some((key, account))));

        let mut dup_cursor = tx.cursor_dup_write::<PlainStorageState>().unwrap();
        let subkey = H256::random();

        let value = U256::from(1);
        let entry1 = StorageEntry { key: subkey, value };
        dup_cursor.upsert(key, entry1).expect(ERROR_UPSERT);
        assert_eq!(dup_cursor.seek_by_key_subkey(key, subkey), Ok(Some(entry1)));

        let value = U256::from(2);
        let entry2 = StorageEntry { key: subkey, value };
        dup_cursor.upsert(key, entry2).expect(ERROR_UPSERT);
        assert_eq!(dup_cursor.seek_by_key_subkey(key, subkey), Ok(Some(entry1)));
        assert_eq!(dup_cursor.next_dup_val(), Ok(Some(entry2)));
    }

    #[test]
    fn db_cursor_dupsort_append() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);

        let transition_id = 2;

        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<AccountChangeSet>().unwrap();
        vec![0, 1, 3, 4, 5]
            .into_iter()
            .try_for_each(|val| {
                cursor.append(
                    transition_id,
                    AccountBeforeTx { address: Address::from_low_u64_be(val), info: None },
                )
            })
            .expect(ERROR_APPEND);
        tx.commit().expect(ERROR_COMMIT);

        // APPEND DUP & APPEND
        let subkey_to_append = 2;
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<AccountChangeSet>().unwrap();
        assert_eq!(
            cursor.append_dup(
                transition_id,
                AccountBeforeTx { address: Address::from_low_u64_be(subkey_to_append), info: None }
            ),
            Err(DatabaseError::Write(-30418))
        );
        assert_eq!(
            cursor.append(
                transition_id - 1,
                AccountBeforeTx { address: Address::from_low_u64_be(subkey_to_append), info: None }
            ),
            Err(DatabaseError::Write(-30418))
        );
        assert_eq!(
            cursor.append(
                transition_id,
                AccountBeforeTx { address: Address::from_low_u64_be(subkey_to_append), info: None }
            ),
            Ok(())
        );
    }

    #[test]
    fn db_closure_put_get() {
        let path = TempDir::new().expect(ERROR_TEMPDIR).into_path();

        let value = Account {
            nonce: 18446744073709551615,
            bytecode_hash: Some(H256::random()),
            balance: U256::MAX,
        };
        let key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        {
            let env = create_test_db_with_path::<WriteMap>(EnvKind::RW, &path);

            // PUT
            let result = env.update(|tx| {
                tx.put::<PlainAccountState>(key, value).expect(ERROR_PUT);
                200
            });
            assert!(result.expect(ERROR_RETURN_VALUE) == 200);
        }

        let env = Env::<WriteMap>::open(&path, EnvKind::RO, None).expect(ERROR_DB_CREATION);

        // GET
        let result =
            env.view(|tx| tx.get::<PlainAccountState>(key).expect(ERROR_GET)).expect(ERROR_GET);

        assert!(result == Some(value))
    }

    #[test]
    fn db_dup_sort() {
        let env = create_test_db::<NoWriteMap>(EnvKind::RW);
        let key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        // PUT (0,0)
        let value00 = StorageEntry::default();
        env.update(|tx| tx.put::<PlainStorageState>(key, value00).expect(ERROR_PUT)).unwrap();

        // PUT (2,2)
        let value22 = StorageEntry { key: H256::from_low_u64_be(2), value: U256::from(2) };
        env.update(|tx| tx.put::<PlainStorageState>(key, value22).expect(ERROR_PUT)).unwrap();

        // PUT (1,1)
        let value11 = StorageEntry { key: H256::from_low_u64_be(1), value: U256::from(1) };
        env.update(|tx| tx.put::<PlainStorageState>(key, value11).expect(ERROR_PUT)).unwrap();

        // Iterate with cursor
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            // Notice that value11 and value22 have been ordered in the DB.
            assert!(Some(value00) == cursor.next_dup_val().unwrap());
            assert!(Some(value11) == cursor.next_dup_val().unwrap());
            assert!(Some(value22) == cursor.next_dup_val().unwrap());
        }

        // Seek value with exact subkey
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            let mut walker = cursor.walk_dup(Some(key), Some(H256::from_low_u64_be(1))).unwrap();
            assert_eq!(
                (key, value11),
                walker
                    .next()
                    .expect("element should exist.")
                    .expect("should be able to retrieve it.")
            );
        }
    }

    #[test]
    fn db_iterate_over_all_dup_values() {
        let env = create_test_db::<NoWriteMap>(EnvKind::RW);
        let key1 = Address::from_str("0x1111111111111111111111111111111111111111")
            .expect(ERROR_ETH_ADDRESS);
        let key2 = Address::from_str("0x2222222222222222222222222222222222222222")
            .expect(ERROR_ETH_ADDRESS);

        // PUT key1 (0,0)
        let value00 = StorageEntry::default();
        env.update(|tx| tx.put::<PlainStorageState>(key1, value00).expect(ERROR_PUT)).unwrap();

        // PUT key1 (1,1)
        let value11 = StorageEntry { key: H256::from_low_u64_be(1), value: U256::from(1) };
        env.update(|tx| tx.put::<PlainStorageState>(key1, value11).expect(ERROR_PUT)).unwrap();

        // PUT key2 (2,2)
        let value22 = StorageEntry { key: H256::from_low_u64_be(2), value: U256::from(2) };
        env.update(|tx| tx.put::<PlainStorageState>(key2, value22).expect(ERROR_PUT)).unwrap();

        // Iterate with walk_dup
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            let mut walker = cursor.walk_dup(None, None).unwrap();

            // Notice that value11 and value22 have been ordered in the DB.
            assert_eq!(Some(Ok((key1, value00))), walker.next());
            assert_eq!(Some(Ok((key1, value11))), walker.next());
            // NOTE: Dup cursor does NOT iterates on all values but only on duplicated values of the
            // same key. assert_eq!(Ok(Some(value22.clone())), walker.next());
            assert_eq!(None, walker.next());
        }

        // Iterate by using `walk`
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            let first = cursor.first().unwrap().unwrap();
            let mut walker = cursor.walk(Some(first.0)).unwrap();
            assert_eq!(Some(Ok((key1, value00))), walker.next());
            assert_eq!(Some(Ok((key1, value11))), walker.next());
            assert_eq!(Some(Ok((key2, value22))), walker.next());
        }
    }

    #[test]
    fn dup_value_with_same_subkey() {
        let env = create_test_db::<NoWriteMap>(EnvKind::RW);
        let key1 = H160([0x11; 20]);
        let key2 = H160([0x22; 20]);

        // PUT key1 (0,1)
        let value01 = StorageEntry { key: H256::from_low_u64_be(0), value: U256::from(1) };
        env.update(|tx| tx.put::<PlainStorageState>(key1, value01).expect(ERROR_PUT)).unwrap();

        // PUT key1 (0,0)
        let value00 = StorageEntry::default();
        env.update(|tx| tx.put::<PlainStorageState>(key1, value00).expect(ERROR_PUT)).unwrap();

        // PUT key2 (2,2)
        let value22 = StorageEntry { key: H256::from_low_u64_be(2), value: U256::from(2) };
        env.update(|tx| tx.put::<PlainStorageState>(key2, value22).expect(ERROR_PUT)).unwrap();

        // Iterate with walk
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            let first = cursor.first().unwrap().unwrap();
            let mut walker = cursor.walk(Some(first.0)).unwrap();

            // NOTE: Both values are present
            assert_eq!(Some(Ok((key1, value00))), walker.next());
            assert_eq!(Some(Ok((key1, value01))), walker.next());
            assert_eq!(Some(Ok((key2, value22))), walker.next());
        }

        // seek_by_key_subkey
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            // NOTE: There are two values with same SubKey but only first one is shown
            assert_eq!(Ok(Some(value00)), cursor.seek_by_key_subkey(key1, value00.key));
            // key1 but value is greater than the one in the DB
            assert_eq!(Ok(None), cursor.seek_by_key_subkey(key1, value22.key));
        }
    }

    #[test]
    fn db_sharded_key() {
        let db: Arc<Env<WriteMap>> = create_test_db(EnvKind::RW);
        let real_key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047").unwrap();

        for i in 1..5 {
            let key = ShardedKey::new(real_key, i * 100);
            let list: IntegerList = vec![i * 100u64].into();

            db.update(|tx| tx.put::<AccountHistory>(key.clone(), list.clone()).expect("")).unwrap();
        }

        // Seek value with non existing key.
        {
            let tx = db.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_read::<AccountHistory>().unwrap();

            // It will seek the one greater or equal to the query. Since we have `Address | 100`,
            // `Address | 200` in the database and we're querying `Address | 150` it will return us
            // `Address | 200`.
            let mut walker = cursor.walk(Some(ShardedKey::new(real_key, 150))).unwrap();
            let (key, list) = walker
                .next()
                .expect("element should exist.")
                .expect("should be able to retrieve it.");

            assert_eq!(ShardedKey::new(real_key, 200), key);
            let list200: IntegerList = vec![200u64].into();
            assert_eq!(list200, list);
        }
        // Seek greatest index
        {
            let tx = db.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_read::<AccountHistory>().unwrap();

            // It will seek the MAX value of transition index and try to use prev to get first
            // biggers.
            let _unknown = cursor.seek_exact(ShardedKey::new(real_key, u64::MAX)).unwrap();
            let (key, list) = cursor
                .prev()
                .expect("element should exist.")
                .expect("should be able to retrieve it.");

            assert_eq!(ShardedKey::new(real_key, 400), key);
            let list400: IntegerList = vec![400u64].into();
            assert_eq!(list400, list);
        }
    }
}
