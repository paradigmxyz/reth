use libc::c_int;
use std::{ffi::CStr, fmt, result, str};

/// An MDBX error kind.
#[derive(Debug)]
pub enum Error {
    KeyExist,
    NotFound,
    NoData,
    PageNotFound,
    Corrupted,
    Panic,
    VersionMismatch,
    Invalid,
    MapFull,
    DbsFull,
    ReadersFull,
    TxnFull,
    CursorFull,
    PageFull,
    UnableExtendMapsize,
    Incompatible,
    BadRslot,
    BadTxn,
    BadValSize,
    BadDbi,
    Problem,
    Busy,
    Multival,
    WannaRecovery,
    KeyMismatch,
    InvalidValue,
    Access,
    TooLarge,
    DecodeError(Box<dyn std::error::Error + Send + Sync + 'static>),
    Other(c_int),
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
            ffi::MDBX_UNABLE_EXTEND_MAPSIZE => Error::UnableExtendMapsize,
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
            ffi::MDBX_EINVAL => Error::InvalidValue,
            ffi::MDBX_EACCESS => Error::Access,
            ffi::MDBX_TOO_LARGE => Error::TooLarge,
            other => Error::Other(other),
        }
    }

    /// Converts an [Error] to the raw error code.
    fn to_err_code(&self) -> c_int {
        match self {
            Error::KeyExist => ffi::MDBX_KEYEXIST,
            Error::NotFound => ffi::MDBX_NOTFOUND,
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
            Error::UnableExtendMapsize => ffi::MDBX_UNABLE_EXTEND_MAPSIZE,
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
            Error::InvalidValue => ffi::MDBX_EINVAL,
            Error::Access => ffi::MDBX_EACCESS,
            Error::TooLarge => ffi::MDBX_TOO_LARGE,
            Error::Other(err_code) => *err_code,
            _ => unreachable!(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::DecodeError(reason) => write!(fmt, "{}", reason),
            other => {
                write!(fmt, "{}", unsafe {
                    let err = ffi::mdbx_strerror(other.to_err_code());
                    str::from_utf8_unchecked(CStr::from_ptr(err).to_bytes())
                })
            }
        }
    }
}

impl std::error::Error for Error {}

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

        assert_eq!(
            "MDBX_INVALID: File is not an MDBX file",
            Error::Invalid.to_string()
        );
    }
}
