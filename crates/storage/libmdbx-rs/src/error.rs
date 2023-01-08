use libc::c_int;
use std::{ffi::CStr, fmt, result, str};

/// An MDBX error kind.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum Error {
    /// The key/value pair already exists.
    KeyExist,
    /// The requested key/value pair was not found.
    NotFound,
    NoData,
    /// The requested page was not found.
    PageNotFound,
    /// The database is corrupted (e.g. a page was a wrong type)
    Corrupted,
    /// The environment had a fatal error (e.g. failed to update a meta page)
    Panic,
    VersionMismatch,
    /// File is not a valid MDBX file
    Invalid,
    /// Environment map size reached.
    MapFull,
    /// Environment reached the maximum number of databases.
    DbsFull,
    /// Environment reached the maximum number of readers.
    ReadersFull,
    /// The transaction has too many dirty pages (i.e. the transaction is too big).
    TxnFull,
    /// The cursor stack is too deep.
    CursorFull,
    /// The page does not have enough space.
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
    UnableExtendMapSize,
    Incompatible,
    BadRslot,
    BadTxn,
    BadValSize,
    BadDbi,
    Problem,
    Busy,
    Multival,
    BadSignature,
    WannaRecovery,
    KeyMismatch,
    DecodeError,
    Access,
    TooLarge,
    DecodeErrorLenDiff,
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
    pub fn to_err_code(&self) -> u32 {
        let err_code = match self {
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
            Error::DecodeError => ffi::MDBX_EINVAL,
            Error::Access => ffi::MDBX_EACCESS,
            Error::TooLarge => ffi::MDBX_TOO_LARGE,
            Error::BadSignature => ffi::MDBX_EBADSIGN,
            Error::Other(err_code) => *err_code,
            _ => unreachable!(),
        };
        err_code as u32
    }
}

impl From<Error> for u32 {
    fn from(value: Error) -> Self {
        value.to_err_code()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let value = match self {
            Self::DecodeErrorLenDiff => "Mismatched data length",
            _ => unsafe {
                let err = ffi::mdbx_strerror(self.to_err_code() as i32);
                str::from_utf8_unchecked(CStr::from_ptr(err).to_bytes())
            },
        };
        write!(fmt, "{value}")
    }
}

/// An MDBX result.
pub type Result<T> = result::Result<T, Error>;

pub fn mdbx_result(err_code: c_int) -> Result<bool> {
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
mod test {
    use super::*;

    #[test]
    fn test_description() {
        assert_eq!("Permission denied", Error::from_err_code(13).to_string());

        assert_eq!("MDBX_INVALID: File is not an MDBX file", Error::Invalid.to_string());
    }

    #[test]
    fn test_conversion() {
        assert_eq!(Error::from_err_code(ffi::MDBX_KEYEXIST), Error::KeyExist);
    }
}
