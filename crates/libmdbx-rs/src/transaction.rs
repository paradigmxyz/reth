use crate::{
    database::Database,
    environment::{Environment, EnvironmentKind, NoWriteMap, TxnManagerMessage, TxnPtr},
    error::{mdbx_result, Result},
    flags::{DatabaseFlags, WriteFlags},
    Cursor, Error, Stat, TableObject,
};
use ffi::{MDBX_txn_flags_t, MDBX_TXN_RDONLY, MDBX_TXN_READWRITE};
use indexmap::IndexSet;
use libc::{c_uint, c_void};
use parking_lot::Mutex;
use std::{
    fmt,
    fmt::Debug,
    marker::PhantomData,
    mem::size_of,
    ptr, result, slice,
    sync::{mpsc::sync_channel, Arc},
};

mod private {
    use super::*;

    pub trait Sealed {}

    impl<'env> Sealed for RO {}
    impl<'env> Sealed for RW {}
}

pub trait TransactionKind: private::Sealed + Debug + 'static {
    #[doc(hidden)]
    const ONLY_CLEAN: bool;

    #[doc(hidden)]
    const OPEN_FLAGS: MDBX_txn_flags_t;
}

#[derive(Debug)]
pub struct RO;
#[derive(Debug)]
pub struct RW;

impl TransactionKind for RO {
    const ONLY_CLEAN: bool = true;
    const OPEN_FLAGS: MDBX_txn_flags_t = MDBX_TXN_RDONLY;
}
impl TransactionKind for RW {
    const ONLY_CLEAN: bool = false;
    const OPEN_FLAGS: MDBX_txn_flags_t = MDBX_TXN_READWRITE;
}

/// An MDBX transaction.
///
/// All database operations require a transaction.
pub struct Transaction<'env, K, E>
where
    K: TransactionKind,
    E: EnvironmentKind,
{
    txn: Arc<Mutex<*mut ffi::MDBX_txn>>,
    primed_dbis: Mutex<IndexSet<ffi::MDBX_dbi>>,
    committed: bool,
    env: &'env Environment<E>,
    _marker: PhantomData<fn(K)>,
}

impl<'env, K, E> Transaction<'env, K, E>
where
    K: TransactionKind,
    E: EnvironmentKind,
{
    pub(crate) fn new(env: &'env Environment<E>) -> Result<Self> {
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        unsafe {
            mdbx_result(ffi::mdbx_txn_begin_ex(
                env.env(),
                ptr::null_mut(),
                K::OPEN_FLAGS,
                &mut txn,
                ptr::null_mut(),
            ))?;
            Ok(Self::new_from_ptr(env, txn))
        }
    }

    pub(crate) fn new_from_ptr(env: &'env Environment<E>, txn: *mut ffi::MDBX_txn) -> Self {
        Self {
            txn: Arc::new(Mutex::new(txn)),
            primed_dbis: Mutex::new(IndexSet::new()),
            committed: false,
            env,
            _marker: PhantomData,
        }
    }

    /// Returns a raw pointer to the underlying MDBX transaction.
    ///
    /// The caller **must** ensure that the pointer is not used after the
    /// lifetime of the transaction.
    pub(crate) fn txn_mutex(&self) -> Arc<Mutex<*mut ffi::MDBX_txn>> {
        self.txn.clone()
    }

    pub fn txn(&self) -> *mut ffi::MDBX_txn {
        *self.txn.lock()
    }

    /// Returns a raw pointer to the MDBX environment.
    pub fn env(&self) -> &Environment<E> {
        self.env
    }

    /// Returns the transaction id.
    pub fn id(&self) -> u64 {
        txn_execute(&self.txn, |txn| unsafe { ffi::mdbx_txn_id(txn) })
    }

    /// Gets an item from a database.
    ///
    /// This function retrieves the data associated with the given key in the
    /// database. If the database supports duplicate keys
    /// ([DatabaseFlags::DUP_SORT]) then the first data item for the key will be
    /// returned. Retrieval of other items requires the use of
    /// [Cursor]. If the item is not in the database, then
    /// [None] will be returned.
    pub fn get<'txn, Key>(&'txn self, db: &Database<'txn>, key: &[u8]) -> Result<Option<Key>>
    where
        Key: TableObject<'txn>,
    {
        let key_val: ffi::MDBX_val = ffi::MDBX_val {
            iov_len: key.len(),
            iov_base: key.as_ptr() as *mut c_void,
        };
        let mut data_val: ffi::MDBX_val = ffi::MDBX_val {
            iov_len: 0,
            iov_base: ptr::null_mut(),
        };

        txn_execute(&self.txn, |txn| unsafe {
            match ffi::mdbx_get(txn, db.dbi(), &key_val, &mut data_val) {
                ffi::MDBX_SUCCESS => Key::decode_val::<K>(txn, &data_val).map(Some),
                ffi::MDBX_NOTFOUND => Ok(None),
                err_code => Err(Error::from_err_code(err_code)),
            }
        })
    }

    /// Commits the transaction.
    ///
    /// Any pending operations will be saved.
    pub fn commit(self) -> Result<bool> {
        self.commit_and_rebind_open_dbs().map(|v| v.0)
    }

    pub fn prime_for_permaopen(&self, db: Database<'_>) {
        self.primed_dbis.lock().insert(db.dbi());
    }

    /// Commits the transaction and returns table handles permanently open for the lifetime of `Environment`.
    pub fn commit_and_rebind_open_dbs(mut self) -> Result<(bool, Vec<Database<'env>>)> {
        let txnlck = self.txn.lock();
        let txn = *txnlck;
        let result = if K::ONLY_CLEAN {
            mdbx_result(unsafe { ffi::mdbx_txn_commit_ex(txn, ptr::null_mut()) })
        } else {
            let (sender, rx) = sync_channel(0);
            self.env
                .txn_manager
                .as_ref()
                .unwrap()
                .send(TxnManagerMessage::Commit {
                    tx: TxnPtr(txn),
                    sender,
                })
                .unwrap();
            rx.recv().unwrap()
        };
        self.committed = true;
        result.map(|v| {
            (
                v,
                self.primed_dbis
                    .lock()
                    .iter()
                    .map(|&dbi| Database::new_from_ptr(dbi))
                    .collect(),
            )
        })
    }

    /// Opens a handle to an MDBX database.
    ///
    /// If `name` is [None], then the returned handle will be for the default database.
    ///
    /// If `name` is not [None], then the returned handle will be for a named database. In this
    /// case the environment must be configured to allow named databases through
    /// [EnvironmentBuilder::set_max_dbs()](crate::EnvironmentBuilder::set_max_dbs).
    ///
    /// The returned database handle may be shared among any transaction in the environment.
    ///
    /// The database name may not contain the null character.
    pub fn open_db<'txn>(&'txn self, name: Option<&str>) -> Result<Database<'txn>> {
        Database::new(self, name, 0)
    }

    /// Gets the option flags for the given database in the transaction.
    pub fn db_flags<'txn>(&'txn self, db: &Database<'txn>) -> Result<DatabaseFlags> {
        let mut flags: c_uint = 0;
        unsafe {
            mdbx_result(txn_execute(&self.txn, |txn| {
                ffi::mdbx_dbi_flags_ex(txn, db.dbi(), &mut flags, ptr::null_mut())
            }))?;
        }
        Ok(DatabaseFlags::from_bits_truncate(flags))
    }

    /// Retrieves database statistics.
    pub fn db_stat<'txn>(&'txn self, db: &Database<'txn>) -> Result<Stat> {
        unsafe {
            let mut stat = Stat::new();
            mdbx_result(txn_execute(&self.txn, |txn| {
                ffi::mdbx_dbi_stat(txn, db.dbi(), stat.mdb_stat(), size_of::<Stat>())
            }))?;
            Ok(stat)
        }
    }

    /// Open a new cursor on the given database.
    pub fn cursor<'txn>(&'txn self, db: &Database<'txn>) -> Result<Cursor<'txn, K>> {
        Cursor::new(self, db)
    }
}

pub(crate) fn txn_execute<F: FnOnce(*mut ffi::MDBX_txn) -> T, T>(
    txn: &Mutex<*mut ffi::MDBX_txn>,
    f: F,
) -> T {
    let lck = txn.lock();
    (f)(*lck)
}

impl<'env, E> Transaction<'env, RW, E>
where
    E: EnvironmentKind,
{
    fn open_db_with_flags<'txn>(
        &'txn self,
        name: Option<&str>,
        flags: DatabaseFlags,
    ) -> Result<Database<'txn>> {
        Database::new(self, name, flags.bits())
    }

    /// Opens a handle to an MDBX database, creating the database if necessary.
    ///
    /// If the database is already created, the given option flags will be added to it.
    ///
    /// If `name` is [None], then the returned handle will be for the default database.
    ///
    /// If `name` is not [None], then the returned handle will be for a named database. In this
    /// case the environment must be configured to allow named databases through
    /// [EnvironmentBuilder::set_max_dbs()](crate::EnvironmentBuilder::set_max_dbs).
    ///
    /// This function will fail with [Error::BadRslot](crate::error::Error::BadRslot) if called by a thread with an open
    /// transaction.
    pub fn create_db<'txn>(
        &'txn self,
        name: Option<&str>,
        flags: DatabaseFlags,
    ) -> Result<Database<'txn>> {
        self.open_db_with_flags(name, flags | DatabaseFlags::CREATE)
    }

    /// Stores an item into a database.
    ///
    /// This function stores key/data pairs in the database. The default
    /// behavior is to enter the new key/data pair, replacing any previously
    /// existing key if duplicates are disallowed, or adding a duplicate data
    /// item if duplicates are allowed ([DatabaseFlags::DUP_SORT]).
    pub fn put<'txn>(
        &'txn self,
        db: &Database<'txn>,
        key: impl AsRef<[u8]>,
        data: impl AsRef<[u8]>,
        flags: WriteFlags,
    ) -> Result<()> {
        let key = key.as_ref();
        let data = data.as_ref();
        let key_val: ffi::MDBX_val = ffi::MDBX_val {
            iov_len: key.len(),
            iov_base: key.as_ptr() as *mut c_void,
        };
        let mut data_val: ffi::MDBX_val = ffi::MDBX_val {
            iov_len: data.len(),
            iov_base: data.as_ptr() as *mut c_void,
        };
        mdbx_result(txn_execute(&self.txn, |txn| unsafe {
            ffi::mdbx_put(txn, db.dbi(), &key_val, &mut data_val, flags.bits())
        }))?;

        Ok(())
    }

    /// Returns a buffer which can be used to write a value into the item at the
    /// given key and with the given length. The buffer must be completely
    /// filled by the caller.
    pub fn reserve<'txn>(
        &'txn self,
        db: &Database<'txn>,
        key: impl AsRef<[u8]>,
        len: usize,
        flags: WriteFlags,
    ) -> Result<&'txn mut [u8]> {
        let key = key.as_ref();
        let key_val: ffi::MDBX_val = ffi::MDBX_val {
            iov_len: key.len(),
            iov_base: key.as_ptr() as *mut c_void,
        };
        let mut data_val: ffi::MDBX_val = ffi::MDBX_val {
            iov_len: len,
            iov_base: ptr::null_mut::<c_void>(),
        };
        unsafe {
            mdbx_result(txn_execute(&self.txn, |txn| {
                ffi::mdbx_put(
                    txn,
                    db.dbi(),
                    &key_val,
                    &mut data_val,
                    flags.bits() | ffi::MDBX_RESERVE,
                )
            }))?;
            Ok(slice::from_raw_parts_mut(
                data_val.iov_base as *mut u8,
                data_val.iov_len,
            ))
        }
    }

    /// Delete items from a database.
    /// This function removes key/data pairs from the database.
    ///
    /// The data parameter is NOT ignored regardless the database does support sorted duplicate data items or not.
    /// If the data parameter is [Some] only the matching data item will be deleted.
    /// Otherwise, if data parameter is [None], any/all value(s) for specified key will be deleted.
    ///
    /// Returns `true` if the key/value pair was present.
    pub fn del<'txn>(
        &'txn self,
        db: &Database<'txn>,
        key: impl AsRef<[u8]>,
        data: Option<&[u8]>,
    ) -> Result<bool> {
        let key = key.as_ref();
        let key_val: ffi::MDBX_val = ffi::MDBX_val {
            iov_len: key.len(),
            iov_base: key.as_ptr() as *mut c_void,
        };
        let data_val: Option<ffi::MDBX_val> = data.map(|data| ffi::MDBX_val {
            iov_len: data.len(),
            iov_base: data.as_ptr() as *mut c_void,
        });

        mdbx_result({
            txn_execute(&self.txn, |txn| {
                if let Some(d) = data_val {
                    unsafe { ffi::mdbx_del(txn, db.dbi(), &key_val, &d) }
                } else {
                    unsafe { ffi::mdbx_del(txn, db.dbi(), &key_val, ptr::null()) }
                }
            })
        })
        .map(|_| true)
        .or_else(|e| match e {
            Error::NotFound => Ok(false),
            other => Err(other),
        })
    }

    /// Empties the given database. All items will be removed.
    pub fn clear_db<'txn>(&'txn self, db: &Database<'txn>) -> Result<()> {
        mdbx_result(txn_execute(&self.txn, |txn| unsafe {
            ffi::mdbx_drop(txn, db.dbi(), false)
        }))?;

        Ok(())
    }

    /// Drops the database from the environment.
    ///
    /// # Safety
    /// Caller must close ALL other [Database] and [Cursor] instances pointing to the same dbi BEFORE calling this function.
    pub unsafe fn drop_db<'txn>(&'txn self, db: Database<'txn>) -> Result<()> {
        mdbx_result(txn_execute(&self.txn, |txn| {
            ffi::mdbx_drop(txn, db.dbi(), true)
        }))?;

        Ok(())
    }
}

impl<'env, E> Transaction<'env, RO, E>
where
    E: EnvironmentKind,
{
    /// Closes the database handle.
    ///
    /// # Safety
    /// Caller must close ALL other [Database] and [Cursor] instances pointing to the same dbi BEFORE calling this function.
    pub unsafe fn close_db(&self, db: Database<'_>) -> Result<()> {
        mdbx_result(ffi::mdbx_dbi_close(self.env.env(), db.dbi()))?;

        Ok(())
    }
}

impl<'env> Transaction<'env, RW, NoWriteMap> {
    /// Begins a new nested transaction inside of this transaction.
    pub fn begin_nested_txn(&mut self) -> Result<Transaction<'_, RW, NoWriteMap>> {
        txn_execute(&self.txn, |txn| {
            let (tx, rx) = sync_channel(0);
            self.env
                .txn_manager
                .as_ref()
                .unwrap()
                .send(TxnManagerMessage::Begin {
                    parent: TxnPtr(txn),
                    flags: RW::OPEN_FLAGS,
                    sender: tx,
                })
                .unwrap();

            rx.recv()
                .unwrap()
                .map(|ptr| Transaction::new_from_ptr(self.env, ptr.0))
        })
    }
}

impl<'env, K, E> fmt::Debug for Transaction<'env, K, E>
where
    K: TransactionKind,
    E: EnvironmentKind,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> result::Result<(), fmt::Error> {
        f.debug_struct("RoTransaction").finish()
    }
}

impl<'env, K, E> Drop for Transaction<'env, K, E>
where
    K: TransactionKind,
    E: EnvironmentKind,
{
    fn drop(&mut self) {
        txn_execute(&self.txn, |txn| {
            if !self.committed {
                if K::ONLY_CLEAN {
                    unsafe {
                        ffi::mdbx_txn_abort(txn);
                    }
                } else {
                    let (sender, rx) = sync_channel(0);
                    self.env
                        .txn_manager
                        .as_ref()
                        .unwrap()
                        .send(TxnManagerMessage::Abort {
                            tx: TxnPtr(txn),
                            sender,
                        })
                        .unwrap();
                    rx.recv().unwrap().unwrap();
                }
            }
        })
    }
}

unsafe impl<'env, K, E> Send for Transaction<'env, K, E>
where
    K: TransactionKind,
    E: EnvironmentKind,
{
}

unsafe impl<'env, K, E> Sync for Transaction<'env, K, E>
where
    K: TransactionKind,
    E: EnvironmentKind,
{
}
