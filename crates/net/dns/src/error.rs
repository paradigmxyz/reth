use crate::tree::TreeRootEntry;

/// Alias for a parse result
pub(crate) type ParseEntryResult<T> = Result<T, ParseDnsEntryError>;

/// Alias for lookup results
pub(crate) type LookupResult<T> = Result<T, LookupError>;

/// Error while parsing a [DnsEntry](crate::tree::DnsEntry)
#[derive(thiserror::Error, Debug)]
pub enum ParseDnsEntryError {
    /// Unknown entry error.
    #[error("unknown entry: {0}")]
    /// Indicates an unknown entry encountered during parsing.
    UnknownEntry(String),
    /// Field not found error.
    #[error("field {0} not found")]
    /// Indicates a field was not found during parsing.
    FieldNotFound(&'static str),
    /// Base64 decoding error.
    #[error("base64 decoding failed: {0}")]
    /// Indicates a failure during Base64 decoding.
    Base64DecodeError(String),
    /// Base32 decoding error.
    #[error("base32 decoding failed: {0}")]
    /// Indicates a failure during Base32 decoding.
    Base32DecodeError(String),
    /// RLP decoding error.
    #[error("{0}")]
    /// Indicates an error during RLP decoding.
    RlpDecodeError(String),
    /// Invalid child hash error in a branch.
    #[error("invalid child hash in branch: {0}")]
    /// Indicates an invalid child hash within a branch.
    InvalidChildHash(String),
    /// Other error.
    #[error("{0}")]
    /// Indicates other unspecified errors.
    Other(String),
}

/// Errors that can happen during lookups
#[derive(thiserror::Error, Debug)]
pub(crate) enum LookupError {
    /// Parse error.
    #[error(transparent)]
    /// Represents errors during parsing.
    Parse(#[from] ParseDnsEntryError),
    /// Invalid root error.
    #[error("failed to verify root {0}")]
    /// Indicates failure while verifying the root entry.
    InvalidRoot(TreeRootEntry),
    /// Request timed out error.
    #[error("request timed out")]
    /// Indicates a timeout occurred during the request.
    RequestTimedOut,
    /// Entry not found error.
    #[error("entry not found")]
    /// Indicates the requested entry was not found.
    EntryNotFound,
}
