use crate::{
    environment::EnvironmentKind,
    error::{mdbx_result, Result},
    transaction::{txn_execute, TransactionKind},
    Transaction,
};
use libc::c_uint;
use std::{ffi::CString, marker::PhantomData, ptr};

/// A handle to an individual database in an environment.
///
/// A database handle denotes the name and parameters of a database in an environment.
#[derive(Debug)]
pub struct Database<'txn> {
    dbi: ffi::MDBX_dbi,
    _marker: PhantomData<&'txn ()>,
}

impl<'txn> Database<'txn> {
    /// Opens a new database handle in the given transaction.
    ///
    /// Prefer using `Environment::open_db`, `Environment::create_db`, `TransactionExt::open_db`,
    /// or `RwTransaction::create_db`.
    pub(crate) fn new<'env, K: TransactionKind, E: EnvironmentKind>(
        txn: &'txn Transaction<'env, K, E>,
        name: Option<&str>,
        flags: c_uint,
    ) -> Result<Self> {
        let c_name = name.map(|n| CString::new(n).unwrap());
        let name_ptr = if let Some(c_name) = &c_name {
            c_name.as_ptr()
        } else {
            ptr::null()
        };
        let mut dbi: ffi::MDBX_dbi = 0;
        mdbx_result(txn_execute(&*txn.txn_mutex(), |txn| unsafe {
            ffi::mdbx_dbi_open(txn, name_ptr, flags, &mut dbi)
        }))?;
        Ok(Self::new_from_ptr(dbi))
    }

    pub(crate) fn new_from_ptr(dbi: ffi::MDBX_dbi) -> Self {
        Self {
            dbi,
            _marker: PhantomData,
        }
    }

    pub(crate) fn freelist_db() -> Self {
        Database {
            dbi: 0,
            _marker: PhantomData,
        }
    }

    /// Returns the underlying MDBX database handle.
    ///
    /// The caller **must** ensure that the handle is not used after the lifetime of the
    /// environment, or after the database has been closed.
    pub fn dbi(&self) -> ffi::MDBX_dbi {
        self.dbi
    }
}

unsafe impl<'txn> Send for Database<'txn> {}
unsafe impl<'txn> Sync for Database<'txn> {}
