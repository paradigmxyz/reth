use crate::{txn_manager::TxnManager, TransactionKind};
use libc::c_int;
use std::result;

/// An MDBX result.
pub type Result<T> = result::Result<T, Error>;

/// An MDBX error kind.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Key/data pair already exists.
    #[error("key/data pair already exists")]
    KeyExist,
    /// No matching key/data pair found.
    #[error("no matching key/data pair found")]
    NotFound,
    /// The cursor is already at the end of data.
    #[error("the cursor is already at the end of data")]
    NoData,
    /// Requested page not found.
    #[error("requested page not found")]
    PageNotFound,
    /// Database is corrupted.
    #[error("database is corrupted")]
    Corrupted,
    /// Fatal environment error.
    #[error("fatal environment error")]
    Panic,
    /// DB version mismatch.
    #[error("DB version mismatch")]
    VersionMismatch,
    /// File is not an MDBX file.
    #[error("file is not an MDBX file")]
    Invalid,
    /// Environment map size limit reached.
    #[error("environment map size limit reached")]
    MapFull,
    /// Too many DBI-handles (maxdbs reached).
    #[error("too many DBI-handles (maxdbs reached)")]
    DbsFull,
    /// Too many readers (maxreaders reached).
    #[error("too many readers (maxreaders reached)")]
    ReadersFull,
    /// Transaction has too many dirty pages (i.e., the transaction is too big).
    #[error("transaction has too many dirty pages (i.e., the transaction is too big)")]
    TxnFull,
    /// Cursor stack limit reached.
    #[error("cursor stack limit reached")]
    CursorFull,
    /// Page has no more space.
    #[error("page has no more space")]
    PageFull,
    /// The database engine was unable to extend mapping, e.g. the address space is unavailable or
    /// busy.
    ///
    /// This can mean:
    /// - The database size was extended by other processes beyond the environment map size, and
    ///   the engine was unable to extend the mapping while starting a read transaction. The
    ///   environment should be re-opened to continue.
    /// - The engine was unable to extend the mapping during a write transaction or an explicit
    ///   call to change the geometry of the environment.
    #[error("database engine was unable to extend mapping")]
    UnableExtendMapSize,
    /// Environment or database is not compatible with the requested operation or flags.
    #[error("environment or database is not compatible with the requested operation or flags")]
    Incompatible,
    /// Invalid reuse of reader locktable slot.
    #[error("invalid reuse of reader locktable slot")]
    BadRslot,
    /// Transaction is not valid for requested operation.
    #[error("transaction is not valid for requested operation")]
    BadTxn,
    /// Invalid size or alignment of key or data for the target database.
    #[error("invalid size or alignment of key or data for the target database")]
    BadValSize,
    /// The specified DBI-handle is invalid.
    #[error("the specified DBI-handle is invalid")]
    BadDbi,
    /// Unexpected internal error.
    #[error("unexpected internal error")]
    Problem,
    /// Another write transaction is running.
    #[error("another write transaction is running")]
    Busy,
    /// The specified key has more than one associated value.
    #[error("the specified key has more than one associated value")]
    Multival,
    /// Wrong signature of a runtime object(s).
    #[error("wrong signature of a runtime object(s)")]
    BadSignature,
    /// Database should be recovered, but cannot be done automatically since it's in read-only
    /// mode.
    #[error("database should be recovered, but cannot be done automatically since it's in read-only mode")]
    WannaRecovery,
    /// The given key value is mismatched to the current cursor position.
    #[error("the given key value is mismatched to the current cursor position")]
    KeyMismatch,
    /// Decode error: An invalid parameter was specified.
    #[error("invalid parameter specified")]
    DecodeError,
    /// The environment opened in read-only.
    #[error("the environment opened in read-only")]
    Access,
    /// Database is too large for the current system.
    #[error("database is too large for the current system")]
    TooLarge,
    /// Decode error length difference:
    ///
    /// An invalid parameter was specified, or the environment has an active write transaction.
    #[error("invalid parameter specified or active write transaction")]
    DecodeErrorLenDiff,
    /// If the [Environment](crate::Environment) was opened with
    /// [EnvironmentKind::WriteMap](crate::EnvironmentKind::WriteMap) flag, nested transactions are
    /// not supported.
    #[error("nested transactions are not supported with WriteMap")]
    NestedTransactionsUnsupportedWithWriteMap,
    /// If the [Environment](crate::Environment) was opened with in read-only mode
    /// [Mode::ReadOnly](crate::flags::Mode::ReadOnly), write transactions can't be opened.
    #[error("write transactions are not supported in read-only mode")]
    WriteTransactionUnsupportedInReadOnlyMode,
    #[error("read transaction has been aborted by the transaction manager")]
    ReadTransactionAborted,
    /// Unknown error code.
    #[error("unknown error code")]
    Other(i32),
}

impl Error {
    /// Converts a raw error code to an [Error].
    pub fn from_err_code(err_code: c_int) -> Error {
        match err_code {
            ffi::MDBX_KEYEXIST => Error::KeyExist,
            ffi::MDBX_NOTFOUND => Error::NotFound,
            ffi::MDBX_ENODATA => Error::NoData,
            ffi::MDBX_PAGE_NOTFOUND => Error::PageNotFound,
            ffi::MDBX_CORRUPTED => Error::Corrupted,
            ffi::MDBX_PANIC => Error::Panic,
            ffi::MDBX_VERSION_MISMATCH => Error::VersionMismatch,
            ffi::MDBX_INVALID => Error::Invalid,
            ffi::MDBX_MAP_FULL => Error::MapFull,
            ffi::MDBX_DBS_FULL => Error::DbsFull,
            ffi::MDBX_READERS_FULL => Error::ReadersFull,
            ffi::MDBX_TXN_FULL => Error::TxnFull,
            ffi::MDBX_CURSOR_FULL => Error::CursorFull,
            ffi::MDBX_PAGE_FULL => Error::PageFull,
            ffi::MDBX_UNABLE_EXTEND_MAPSIZE => Error::UnableExtendMapSize,
            ffi::MDBX_INCOMPATIBLE => Error::Incompatible,
            ffi::MDBX_BAD_RSLOT => Error::BadRslot,
            ffi::MDBX_BAD_TXN => Error::BadTxn,
            ffi::MDBX_BAD_VALSIZE => Error::BadValSize,
            ffi::MDBX_BAD_DBI => Error::BadDbi,
            ffi::MDBX_PROBLEM => Error::Problem,
            ffi::MDBX_BUSY => Error::Busy,
            ffi::MDBX_EMULTIVAL => Error::Multival,
            ffi::MDBX_WANNA_RECOVERY => Error::WannaRecovery,
            ffi::MDBX_EKEYMISMATCH => Error::KeyMismatch,
            ffi::MDBX_EINVAL => Error::DecodeError,
            ffi::MDBX_EACCESS => Error::Access,
            ffi::MDBX_TOO_LARGE => Error::TooLarge,
            ffi::MDBX_EBADSIGN => Error::BadSignature,
            other => Error::Other(other),
        }
    }

    /// Converts an [Error] to the raw error code.
    pub fn to_err_code(&self) -> i32 {
        match self {
            Error::KeyExist => ffi::MDBX_KEYEXIST,
            Error::NotFound => ffi::MDBX_NOTFOUND,
            Error::NoData => ffi::MDBX_ENODATA,
            Error::PageNotFound => ffi::MDBX_PAGE_NOTFOUND,
            Error::Corrupted => ffi::MDBX_CORRUPTED,
            Error::Panic => ffi::MDBX_PANIC,
            Error::VersionMismatch => ffi::MDBX_VERSION_MISMATCH,
            Error::Invalid => ffi::MDBX_INVALID,
            Error::MapFull => ffi::MDBX_MAP_FULL,
            Error::DbsFull => ffi::MDBX_DBS_FULL,
            Error::ReadersFull => ffi::MDBX_READERS_FULL,
            Error::TxnFull => ffi::MDBX_TXN_FULL,
            Error::CursorFull => ffi::MDBX_CURSOR_FULL,
            Error::PageFull => ffi::MDBX_PAGE_FULL,
            Error::UnableExtendMapSize => ffi::MDBX_UNABLE_EXTEND_MAPSIZE,
            Error::Incompatible => ffi::MDBX_INCOMPATIBLE,
            Error::BadRslot => ffi::MDBX_BAD_RSLOT,
            Error::BadTxn => ffi::MDBX_BAD_TXN,
            Error::BadValSize => ffi::MDBX_BAD_VALSIZE,
            Error::BadDbi => ffi::MDBX_BAD_DBI,
            Error::Problem => ffi::MDBX_PROBLEM,
            Error::Busy => ffi::MDBX_BUSY,
            Error::Multival => ffi::MDBX_EMULTIVAL,
            Error::WannaRecovery => ffi::MDBX_WANNA_RECOVERY,
            Error::KeyMismatch => ffi::MDBX_EKEYMISMATCH,
            Error::DecodeErrorLenDiff | Error::DecodeError => ffi::MDBX_EINVAL,
            Error::Access => ffi::MDBX_EACCESS,
            Error::TooLarge => ffi::MDBX_TOO_LARGE,
            Error::BadSignature | Error::ReadTransactionAborted => ffi::MDBX_EBADSIGN,
            Error::WriteTransactionUnsupportedInReadOnlyMode => ffi::MDBX_EACCESS,
            Error::NestedTransactionsUnsupportedWithWriteMap => ffi::MDBX_EACCESS,
            Error::Other(err_code) => *err_code,
        }
    }
}

impl From<Error> for i32 {
    fn from(value: Error) -> Self {
        value.to_err_code()
    }
}

#[inline]
pub(crate) fn mdbx_result(err_code: c_int) -> Result<bool> {
    match err_code {
        ffi::MDBX_SUCCESS => Ok(false),
        ffi::MDBX_RESULT_TRUE => Ok(true),
        other => Err(Error::from_err_code(other)),
    }
}

#[cfg(feature = "read-tx-timeouts")]
#[inline]
pub(crate) fn mdbx_result_with_tx_kind<K: TransactionKind>(
    err_code: c_int,
    txn: *mut ffi::MDBX_txn,
    txn_manager: &TxnManager,
) -> Result<bool> {
    if K::IS_READ_ONLY &&
        err_code == ffi::MDBX_EBADSIGN &&
        txn_manager.remove_aborted_read_transaction(txn).is_some()
    {
        return Err(Error::ReadTransactionAborted);
    }

    mdbx_result(err_code)
}

#[cfg(not(feature = "read-tx-timeouts"))]
#[inline]
pub(crate) fn mdbx_result_with_tx_kind<K: TransactionKind>(
    err_code: c_int,
    _txn: *mut ffi::MDBX_txn,
    _txn_manager: &TxnManager,
) -> Result<bool> {
    mdbx_result(err_code)
}

#[macro_export]
macro_rules! mdbx_try_optional {
    ($expr:expr) => {{
        match $expr {
            Err(Error::NotFound | Error::NoData) => return Ok(None),
            Err(e) => return Err(e),
            Ok(v) => v,
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_description() {
        assert_eq!("the environment opened in read-only", Error::from_err_code(13).to_string());

        assert_eq!("file is not an MDBX file", Error::Invalid.to_string());
    }

    #[test]
    fn test_conversion() {
        assert_eq!(Error::from_err_code(ffi::MDBX_KEYEXIST), Error::KeyExist);
    }
}
