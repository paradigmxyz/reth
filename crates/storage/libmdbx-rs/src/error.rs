use std::{ffi::c_int, result};

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
    #[error("the environment opened in read-only, check <https://reth.rs/run/troubleshooting.html> for more")]
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
    /// [`EnvironmentKind::WriteMap`](crate::EnvironmentKind::WriteMap) flag, nested transactions
    /// are not supported.
    #[error("nested transactions are not supported with WriteMap")]
    NestedTransactionsUnsupportedWithWriteMap,
    /// If the [Environment](crate::Environment) was opened with in read-only mode
    /// [`Mode::ReadOnly`](crate::flags::Mode::ReadOnly), write transactions can't be opened.
    #[error("write transactions are not supported in read-only mode")]
    WriteTransactionUnsupportedInReadOnlyMode,
    /// Read transaction has been timed out.
    #[error("read transaction has been timed out")]
    ReadTransactionTimeout,
    /// Permission defined
    #[error("permission denied to setup database")]
    Permission,
    /// Unknown error code.
    #[error("unknown error code: {0}")]
    Other(i32),
}

impl Error {
    /// Converts a raw error code to an [Error].
    pub const fn from_err_code(err_code: c_int) -> Self {
        match err_code {
            ffi::MDBX_KEYEXIST => Self::KeyExist,
            ffi::MDBX_NOTFOUND => Self::NotFound,
            ffi::MDBX_ENODATA => Self::NoData,
            ffi::MDBX_PAGE_NOTFOUND => Self::PageNotFound,
            ffi::MDBX_CORRUPTED => Self::Corrupted,
            ffi::MDBX_PANIC => Self::Panic,
            ffi::MDBX_VERSION_MISMATCH => Self::VersionMismatch,
            ffi::MDBX_INVALID => Self::Invalid,
            ffi::MDBX_MAP_FULL => Self::MapFull,
            ffi::MDBX_DBS_FULL => Self::DbsFull,
            ffi::MDBX_READERS_FULL => Self::ReadersFull,
            ffi::MDBX_TXN_FULL => Self::TxnFull,
            ffi::MDBX_CURSOR_FULL => Self::CursorFull,
            ffi::MDBX_PAGE_FULL => Self::PageFull,
            ffi::MDBX_UNABLE_EXTEND_MAPSIZE => Self::UnableExtendMapSize,
            ffi::MDBX_INCOMPATIBLE => Self::Incompatible,
            ffi::MDBX_BAD_RSLOT => Self::BadRslot,
            ffi::MDBX_BAD_TXN => Self::BadTxn,
            ffi::MDBX_BAD_VALSIZE => Self::BadValSize,
            ffi::MDBX_BAD_DBI => Self::BadDbi,
            ffi::MDBX_PROBLEM => Self::Problem,
            ffi::MDBX_BUSY => Self::Busy,
            ffi::MDBX_EMULTIVAL => Self::Multival,
            ffi::MDBX_WANNA_RECOVERY => Self::WannaRecovery,
            ffi::MDBX_EKEYMISMATCH => Self::KeyMismatch,
            ffi::MDBX_EINVAL => Self::DecodeError,
            ffi::MDBX_EACCESS => Self::Access,
            ffi::MDBX_TOO_LARGE => Self::TooLarge,
            ffi::MDBX_EBADSIGN => Self::BadSignature,
            ffi::MDBX_EPERM => Self::Permission,
            other => Self::Other(other),
        }
    }

    /// Converts an [Error] to the raw error code.
    pub const fn to_err_code(&self) -> i32 {
        match self {
            Self::KeyExist => ffi::MDBX_KEYEXIST,
            Self::NotFound => ffi::MDBX_NOTFOUND,
            Self::NoData => ffi::MDBX_ENODATA,
            Self::PageNotFound => ffi::MDBX_PAGE_NOTFOUND,
            Self::Corrupted => ffi::MDBX_CORRUPTED,
            Self::Panic => ffi::MDBX_PANIC,
            Self::VersionMismatch => ffi::MDBX_VERSION_MISMATCH,
            Self::Invalid => ffi::MDBX_INVALID,
            Self::MapFull => ffi::MDBX_MAP_FULL,
            Self::DbsFull => ffi::MDBX_DBS_FULL,
            Self::ReadersFull => ffi::MDBX_READERS_FULL,
            Self::TxnFull => ffi::MDBX_TXN_FULL,
            Self::CursorFull => ffi::MDBX_CURSOR_FULL,
            Self::PageFull => ffi::MDBX_PAGE_FULL,
            Self::UnableExtendMapSize => ffi::MDBX_UNABLE_EXTEND_MAPSIZE,
            Self::Incompatible => ffi::MDBX_INCOMPATIBLE,
            Self::BadRslot => ffi::MDBX_BAD_RSLOT,
            Self::BadTxn => ffi::MDBX_BAD_TXN,
            Self::BadValSize => ffi::MDBX_BAD_VALSIZE,
            Self::BadDbi => ffi::MDBX_BAD_DBI,
            Self::Problem => ffi::MDBX_PROBLEM,
            Self::Busy => ffi::MDBX_BUSY,
            Self::Multival => ffi::MDBX_EMULTIVAL,
            Self::WannaRecovery => ffi::MDBX_WANNA_RECOVERY,
            Self::KeyMismatch => ffi::MDBX_EKEYMISMATCH,
            Self::DecodeErrorLenDiff | Self::DecodeError => ffi::MDBX_EINVAL,
            Self::TooLarge => ffi::MDBX_TOO_LARGE,
            Self::BadSignature => ffi::MDBX_EBADSIGN,
            Self::Access |
            Self::WriteTransactionUnsupportedInReadOnlyMode |
            Self::NestedTransactionsUnsupportedWithWriteMap => ffi::MDBX_EACCESS,
            Self::ReadTransactionTimeout => -96000, // Custom non-MDBX error code
            Self::Permission => ffi::MDBX_EPERM,
            Self::Other(err_code) => *err_code,
        }
    }
}

impl From<Error> for i32 {
    fn from(value: Error) -> Self {
        value.to_err_code()
    }
}

#[inline]
pub(crate) const fn mdbx_result(err_code: c_int) -> Result<bool> {
    match err_code {
        ffi::MDBX_SUCCESS => Ok(false),
        ffi::MDBX_RESULT_TRUE => Ok(true),
        other => Err(Error::from_err_code(other)),
    }
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
        assert_eq!(
            "the environment opened in read-only, check <https://reth.rs/run/troubleshooting.html> for more",
            Error::from_err_code(13).to_string()
        );

        assert_eq!("file is not an MDBX file", Error::Invalid.to_string());
    }

    #[test]
    fn test_conversion() {
        assert_eq!(Error::from_err_code(ffi::MDBX_KEYEXIST), Error::KeyExist);
    }
}
