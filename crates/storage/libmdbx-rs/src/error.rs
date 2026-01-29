//! Error types and result handling for MDBX operations.

use std::{convert::Infallible, ffi::c_int, result};

/// An MDBX result.
pub type MdbxResult<T, E = MdbxError> = result::Result<T, E>;

/// Result type for codec operations.
pub type ReadResult<T, E = ReadError> = Result<T, E>;

/// Error type for reading from the database.
///
/// This encapsulates errors that can occur during DB operations via
/// `Self::Mdbx` as well as post-read during the [`TableObject`] decoding
/// step via `Self::Decoding`.
///
/// For simplicity, the decoding error is boxed. [`Self::decoding`] can be used
/// to create such an error from any error type fits the bounds. E.g.
/// `result.map_err(ReadError::decoding)`.
///
/// [`TableObject`]: crate::codec::TableObject
#[derive(thiserror::Error, Debug)]
pub enum ReadError {
    /// Mdbx error during decoding.
    #[error(transparent)]
    Mdbx(#[from] MdbxError),
    /// Type-associated error while decoding.
    #[error(transparent)]
    Decoding(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl ReadError {
    /// Creates a new decoding error from a boxed error.
    pub fn decoding<E>(err: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Decoding(Box::new(err))
    }
}

impl From<ReadError> for i32 {
    fn from(err: ReadError) -> Self {
        match err {
            ReadError::Mdbx(e) => e.into(),
            // Use a dedicated error code for decoding errors
            ReadError::Decoding(_) => -1,
        }
    }
}

/// An MDBX error kind.
///
/// This represents various error conditions that can occur when interacting
/// with the MDBX database.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum MdbxError {
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
    /// The database engine was unable to extend mapping, e.g. the address
    /// space is unavailable or busy.
    ///
    /// This can mean:
    ///
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
    /// Database should be recovered, but cannot be done automatically since
    /// it's in read-only mode.
    #[error(
        "database should be recovered, but cannot be done automatically since it's in read-only mode"
    )]
    WannaRecovery,
    /// The given key value is mismatched to the current cursor position.
    #[error("the given key value is mismatched to the current cursor position")]
    KeyMismatch,
    /// Decode error: An invalid parameter was specified.
    #[error("invalid parameter specified")]
    DecodeError,
    /// The environment opened in read-only.
    #[error(
        "the environment opened in read-only, check <https://reth.rs/run/troubleshooting.html> for more"
    )]
    Access,
    /// Database is too large for the current system.
    #[error("database is too large for the current system")]
    TooLarge,
    /// Decode error length difference:
    ///
    /// An invalid parameter was specified, or the environment has an active
    /// write transaction.
    #[error("invalid parameter specified or active write transaction")]
    DecodeErrorLenDiff,
    /// If the [Environment](crate::Environment) was opened with
    /// [`EnvironmentKind::WriteMap`](crate::sys::EnvironmentKind::WriteMap) flag,
    /// nested transactions are not supported.
    #[error("nested transactions are not supported with WriteMap")]
    NestedTransactionsUnsupportedWithWriteMap,
    /// If the [Environment](crate::Environment) was opened with in read-only
    /// mode [`Mode::ReadOnly`](crate::flags::Mode::ReadOnly), write
    /// transactions can't be opened.
    #[error("write transactions are not supported in read-only mode")]
    WriteTransactionUnsupportedInReadOnlyMode,
    /// Read transaction has been timed out.
    #[error("read transaction has been timed out")]
    ReadTransactionTimeout,
    /// The transaction commit was aborted due to previous errors.
    ///
    /// This can happen in exceptionally rare cases and it signals the problem
    /// coming from inside of mdbx.
    #[error("botched transaction")]
    BotchedTransaction,
    /// Permission defined
    #[error("permission denied to setup database")]
    Permission,
    /// Unknown error code.
    #[error("unknown error code: {0}")]
    Other(i32),
    /// Operation requires DUP_SORT flag on database.
    #[error("operation requires DUP_SORT flag on database")]
    RequiresDupSort,
    /// Operation requires DUP_FIXED flag on database.
    #[error("operation requires DUP_FIXED flag on database")]
    RequiresDupFixed,
}

impl MdbxError {
    /// Converts a raw error code to an [`MdbxError`].
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

    /// Converts an [`MdbxError`] to the raw error code.
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
            Self::BotchedTransaction => -96001,
            Self::RequiresDupSort => -96002,
            Self::RequiresDupFixed => -96003,
            Self::Permission => ffi::MDBX_EPERM,
            Self::Other(err_code) => *err_code,
        }
    }
}

impl From<MdbxError> for i32 {
    fn from(value: MdbxError) -> Self {
        value.to_err_code()
    }
}

/// Parses an MDBX error code into a result type.
///
/// Note that this function returns `Ok(false)` on `MDBX_SUCCESS` and
/// `Ok(true)` on `MDBX_RESULT_TRUE`. The return value requires extra
/// care since its interpretation depends on the callee being called.
///
/// The most unintuitive case is `mdbx_txn_commit` which returns `Ok(true)`
/// when the commit has been aborted.
#[inline]
pub(crate) const fn mdbx_result(err_code: c_int) -> MdbxResult<bool> {
    match err_code {
        ffi::MDBX_SUCCESS => Ok(false),
        ffi::MDBX_RESULT_TRUE => Ok(true),
        other => Err(MdbxError::from_err_code(other)),
    }
}

impl From<MdbxError> for Infallible {
    fn from(_value: MdbxError) -> Self {
        unreachable!()
    }
}

/// Parses an MDBX error code into a result type.
///
/// This function returns `Ok(())` on both `MDBX_SUCCESS` and
/// `MDBX_RESULT_TRUE`, effectively treating them both as non-error outcomes.
/// This is useful in scenarios where the distinction between these two
/// success codes is not relevant to the caller, e.g. on a `get` operation
/// where either outcome indicates a successful operation.
#[inline]
#[allow(dead_code)]
pub(crate) const fn mdbx_result_unit(err_code: c_int) -> MdbxResult<()> {
    match err_code {
        ffi::MDBX_SUCCESS => Ok(()),
        ffi::MDBX_RESULT_TRUE => Ok(()),
        other => Err(MdbxError::from_err_code(other)),
    }
}

/// Unwrap a `Result<Option<T>, MdbxError>`, or return `Ok(None)` if the error
/// is `NotFound` or `NoData`.
#[macro_export]
macro_rules! mdbx_try_optional {
    ($expr:expr) => {{
        match $expr {
            Err(MdbxError::NotFound | MdbxError::NoData) => return Ok(None),
            Err(e) => return Err(e),
            Ok(v) => v,
        }
    }};
}

/// Like `mdbx_try_optional!` but for [`ReadError`].
#[macro_export]
macro_rules! codec_try_optional {
    ($expr:expr) => {{
        match $expr {
            Err($crate::error::ReadError::Mdbx(
                $crate::MdbxError::NotFound | $crate::MdbxError::NoData,
            )) => {
                return Ok(None);
            }
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
            MdbxError::from_err_code(13).to_string()
        );

        assert_eq!("file is not an MDBX file", MdbxError::Invalid.to_string());
    }

    #[test]
    fn test_conversion() {
        assert_eq!(MdbxError::from_err_code(ffi::MDBX_KEYEXIST), MdbxError::KeyExist);
    }
}
