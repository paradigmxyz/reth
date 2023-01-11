use crate::tree::{LinkEntry, TreeRootEntry};
use enr::EnrKeyUnambiguous;

/// Alias for a parse result
pub(crate) type ParseEntryResult<T> = Result<T, ParseDnsEntryError>;

pub(crate) type LookupResult<T, K> = Result<T, LookupError<K>>;

/// Error while parsing a [DnsEntry]
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum ParseDnsEntryError {
    #[error("Unknown entry: {0}")]
    UnknownEntry(String),
    #[error("Field {0} not found.")]
    FieldNotFound(&'static str),
    #[error("Base64 decoding failed: {0}")]
    Base64DecodeError(String),
    #[error("Base32 decoding failed: {0}")]
    Base32DecodeError(String),
    #[error("{0}")]
    RlpDecodeError(String),
    #[error("{0}")]
    Other(String),
}

/// Errors that can happen during lookups
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub(crate) enum LookupError<K: EnrKeyUnambiguous> {
    #[error(transparent)]
    Parse(#[from] ParseDnsEntryError),
    #[error("Failed to verify root {0} with link {1}")]
    InvalidRoot(TreeRootEntry, LinkEntry<K>),
    #[error("Request timed out")]
    RequestTimedOut,
}
