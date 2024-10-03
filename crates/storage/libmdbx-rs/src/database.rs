use crate::{
    error::{mdbx_result, Result},
    transaction::TransactionKind,
    Environment, Transaction,
};
use ffi::MDBX_db_flags_t;
use std::{ffi::CStr, ptr};

/// A handle to an individual database in an environment.
///
/// A database handle denotes the name and parameters of a database in an environment.
#[derive(Debug)]
pub struct Database {
    dbi: ffi::MDBX_dbi,
    /// The environment that this database belongs to keeps it alive as long as the database
    /// instance exists.
    _env: Option<Environment>,
}

impl Database {
    /// Opens a new database handle in the given transaction.
    ///
    /// Prefer using `Environment::open_db`, `Environment::create_db`, `TransactionExt::open_db`,
    /// or `RwTransaction::create_db`.
    pub(crate) fn new<K: TransactionKind>(
        txn: &Transaction<K>,
        name: Option<&str>,
        flags: MDBX_db_flags_t,
    ) -> Result<Self> {
        let mut c_name_buf = smallvec::SmallVec::<[u8; 32]>::new();
        let c_name = name.map(|n| {
            c_name_buf.extend_from_slice(n.as_bytes());
            c_name_buf.push(0);
            CStr::from_bytes_with_nul(&c_name_buf).unwrap()
        });
        let name_ptr = if let Some(c_name) = c_name { c_name.as_ptr() } else { ptr::null() };
        let mut dbi: ffi::MDBX_dbi = 0;
        txn.txn_execute(|txn_ptr| {
            mdbx_result(unsafe { ffi::mdbx_dbi_open(txn_ptr, name_ptr, flags, &mut dbi) })
        })??;
        Ok(Self::new_from_ptr(dbi, txn.env().clone()))
    }

    pub(crate) const fn new_from_ptr(dbi: ffi::MDBX_dbi, env: Environment) -> Self {
        Self { dbi, _env: Some(env) }
    }

    /// Opens the freelist database with DBI `0`.
    pub const fn freelist_db() -> Self {
        Self { dbi: 0, _env: None }
    }

    /// Returns the underlying MDBX database handle.
    ///
    /// The caller **must** ensure that the handle is not used after the lifetime of the
    /// environment, or after the database has been closed.
    pub const fn dbi(&self) -> ffi::MDBX_dbi {
        self.dbi
    }
}

unsafe impl Send for Database {}
unsafe impl Sync for Database {}
