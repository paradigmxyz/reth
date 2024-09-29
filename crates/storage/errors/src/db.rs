use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::{
    fmt,
    fmt::{Debug, Display},
    str::FromStr,
};

/// Database error type.
#[derive(Clone, Debug, PartialEq, Eq, derive_more::Display)]
pub enum DatabaseError {
    /// Failed to open the database.
    #[display("failed to open the database: {_0}")]
    Open(DatabaseErrorInfo),
    /// Failed to create a table in the database.
    #[display("failed to create a table: {_0}")]
    CreateTable(DatabaseErrorInfo),
    /// Failed to write a value into a table.
    Write(Box<DatabaseWriteError>),
    /// Failed to read a value from a table.
    #[display("failed to read a value from a database table: {_0}")]
    Read(DatabaseErrorInfo),
    /// Failed to delete a `(key, value)` pair from a table.
    #[display("database delete error code: {_0}")]
    Delete(DatabaseErrorInfo),
    /// Failed to commit transaction changes into the database.
    #[display("failed to commit transaction changes: {_0}")]
    Commit(DatabaseErrorInfo),
    /// Failed to initiate a transaction.
    #[display("failed to initialize a transaction: {_0}")]
    InitTx(DatabaseErrorInfo),
    /// Failed to initialize a cursor.
    #[display("failed to initialize a cursor: {_0}")]
    InitCursor(DatabaseErrorInfo),
    /// Failed to decode a key from a table.
    #[display("failed to decode a key from a table")]
    Decode,
    /// Failed to get database stats.
    #[display("failed to get stats: {_0}")]
    Stats(DatabaseErrorInfo),
    /// Failed to use the specified log level, as it's not available.
    #[display("log level {_0:?} is not available")]
    LogLevelUnavailable(LogLevel),
    /// Other unspecified error.
    #[display("{_0}")]
    Other(String),
}

impl core::error::Error for DatabaseError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Write(err) => core::error::Error::source(err),
            _ => Option::None,
        }
    }
}

/// Common error struct to propagate implementation-specific error information.
#[derive(Debug, Clone, PartialEq, Eq, derive_more::Display)]
#[display("{message} ({code})")]
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
    fn from(error: E) -> Self {
        Self { message: error.to_string(), code: error.into() }
    }
}

impl From<DatabaseWriteError> for DatabaseError {
    #[inline]
    fn from(error: DatabaseWriteError) -> Self {
        Self::Write(Box::new(error))
    }
}

/// Database write error.
#[derive(Clone, Debug, PartialEq, Eq)]
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

impl fmt::Display for DatabaseWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "write operation {:?} failed for key \"{}\" in table {}: {}",
            self.operation,
            alloy_primitives::hex::encode(&self.key),
            self.table_name,
            self.info
        )
    }
}

impl core::error::Error for DatabaseWriteError {}

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
