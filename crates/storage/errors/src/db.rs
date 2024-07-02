#[cfg(feature = "std")]
use std::{fmt::Display, str::FromStr, string::String};

#[cfg(not(feature = "std"))]
use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec::Vec,
};

#[cfg(not(feature = "std"))]
use core::{fmt::Display, str::FromStr};

/// Database error type.
#[derive(Clone, Debug, PartialEq, Eq, thiserror_no_std::Error)]
pub enum DatabaseError {
    /// Failed to open the database.
    #[error("failed to open the database: {0}")]
    Open(DatabaseErrorInfo),
    /// Failed to create a table in the database.
    #[error("failed to create a table: {0}")]
    CreateTable(DatabaseErrorInfo),
    /// Failed to write a value into a table.
    #[error(transparent)]
    Write(#[from] Box<DatabaseWriteError>),
    /// Failed to read a value from a table.
    #[error("failed to read a value from a database table: {0}")]
    Read(DatabaseErrorInfo),
    /// Failed to delete a `(key, value)` pair from a table.
    #[error("database delete error code: {0}")]
    Delete(DatabaseErrorInfo),
    /// Failed to commit transaction changes into the database.
    #[error("failed to commit transaction changes: {0}")]
    Commit(DatabaseErrorInfo),
    /// Failed to initiate a transaction.
    #[error("failed to initialize a transaction: {0}")]
    InitTx(DatabaseErrorInfo),
    /// Failed to initialize a cursor.
    #[error("failed to initialize a cursor: {0}")]
    InitCursor(DatabaseErrorInfo),
    /// Failed to decode a key from a table.
    #[error("failed to decode a key from a table")]
    Decode,
    /// Failed to get database stats.
    #[error("failed to get stats: {0}")]
    Stats(DatabaseErrorInfo),
    /// Failed to use the specified log level, as it's not available.
    #[error("log level {0:?} is not available")]
    LogLevelUnavailable(LogLevel),
    /// Other unspecified error.
    #[error("{0}")]
    Other(String),
}

/// Common error struct to propagate implementation-specific error information.
#[derive(Debug, Clone, PartialEq, Eq, thiserror_no_std::Error)]
#[error("{message} ({code})")]
pub struct DatabaseErrorInfo {
    /// Human-readable error message.
    pub message: String,
    /// Error code.
    pub code: i32,
}

impl<E> From<E> for DatabaseErrorInfo
where
    E: Display + Into<i32>,
{
    #[inline]
    fn from(value: E) -> Self {
        Self { message: value.to_string(), code: value.into() }
    }
}

impl From<DatabaseWriteError> for DatabaseError {
    #[inline]
    fn from(value: DatabaseWriteError) -> Self {
        Self::Write(Box::new(value))
    }
}

/// Database write error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror_no_std::Error)]
#[error(
    "write operation {operation:?} failed for key \"{key}\" in table {table_name:?}: {info}",
    key = reth_primitives::hex::encode(key),
)]
pub struct DatabaseWriteError {
    /// The error code and message.
    pub info: DatabaseErrorInfo,
    /// The write operation type.
    pub operation: DatabaseWriteOperation,
    /// The table name.
    pub table_name: &'static str,
    /// The write key.
    pub key: Vec<u8>,
}

/// Database write operation type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatabaseWriteOperation {
    /// Append cursor.
    CursorAppend,
    /// Upsert cursor.
    CursorUpsert,
    /// Insert cursor.
    CursorInsert,
    /// Append duplicate cursor.
    CursorAppendDup,
    /// Put.
    Put,
}

/// Database log level.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum LogLevel {
    /// Enables logging for critical conditions, i.e. assertion failures.
    Fatal,
    /// Enables logging for error conditions.
    Error,
    /// Enables logging for warning conditions.
    Warn,
    /// Enables logging for normal but significant condition.
    Notice,
    /// Enables logging for verbose informational.
    Verbose,
    /// Enables logging for debug-level messages.
    Debug,
    /// Enables logging for trace debug-level messages.
    Trace,
    /// Enables logging for extra debug-level messages.
    Extra,
}

impl LogLevel {
    /// All possible variants of the `LogLevel` enum
    pub const fn value_variants() -> &'static [Self] {
        &[
            Self::Fatal,
            Self::Error,
            Self::Warn,
            Self::Notice,
            Self::Verbose,
            Self::Debug,
            Self::Trace,
            Self::Extra,
        ]
    }

    /// Static str reference to `LogLevel` enum, required for `Clap::Builder::PossibleValue::new()`
    pub const fn variant_name(&self) -> &'static str {
        match self {
            Self::Fatal => "fatal",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Notice => "notice",
            Self::Verbose => "verbose",
            Self::Debug => "debug",
            Self::Trace => "trace",
            Self::Extra => "extra",
        }
    }

    /// Returns all variants descriptions
    pub const fn help_message(&self) -> &'static str {
        match self {
            Self::Fatal => "Enables logging for critical conditions, i.e. assertion failures",
            Self::Error => "Enables logging for error conditions",
            Self::Warn => "Enables logging for warning conditions",
            Self::Notice => "Enables logging for normal but significant condition",
            Self::Verbose => "Enables logging for verbose informational",
            Self::Debug => "Enables logging for debug-level messages",
            Self::Trace => "Enables logging for trace debug-level messages",
            Self::Extra => "Enables logging for extra debug-level messages",
        }
    }
}

impl FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "fatal" => Ok(Self::Fatal),
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "notice" => Ok(Self::Notice),
            "verbose" => Ok(Self::Verbose),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            "extra" => Ok(Self::Extra),
            _ => Err(format!("Invalid log level: {s}")),
        }
    }
}
