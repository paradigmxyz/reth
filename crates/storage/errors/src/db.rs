use core::{
    fmt::{Display, Formatter, Result},
    str::FromStr,
};

extern crate alloc;
use alloc::boxed::Box;

/// Database error type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DatabaseError {
    /// Failed to open the database.
    Open(DatabaseErrorInfo),
    /// Failed to create a table in the database.
    CreateTable(DatabaseErrorInfo),
    /// Failed to write a value into a table.
    Write(Box<DatabaseWriteError>),
    /// Failed to read a value from a table.
    Read(DatabaseErrorInfo),
    /// Failed to delete a `(key, value)` pair from a table.
    Delete(DatabaseErrorInfo),
    /// Failed to commit transaction changes into the database.
    Commit(DatabaseErrorInfo),
    /// Failed to initiate a transaction.
    InitTx(DatabaseErrorInfo),
    /// Failed to initialize a cursor.
    InitCursor(DatabaseErrorInfo),
    /// Failed to decode a key from a table.
    Decode,
    /// Failed to get database stats.
    Stats(DatabaseErrorInfo),
    /// Failed to use the specified log level, as it's not available.
    LogLevelUnavailable(LogLevel),
    /// Other unspecified error.
    Other(String),
}

#[cfg(feature = "std")]
impl std::error::Error for DatabaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Write(err) => Some(&**err),
            _ => None,
        }
    }
}

impl Display for DatabaseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::Open(info) => write!(f, "failed to open the database: {}", info),
            Self::CreateTable(info) => write!(f, "failed to create a table: {}", info),
            Self::Write(err) => write!(f, "{}", err),
            Self::Read(info) => write!(f, "failed to read a value from a database table: {}", info),
            Self::Delete(info) => write!(f, "database delete error code: {}", info),
            Self::Commit(info) => write!(f, "failed to commit transaction changes: {}", info),
            Self::InitTx(info) => write!(f, "failed to initialize a transaction: {}", info),
            Self::InitCursor(info) => write!(f, "failed to initialize a cursor: {}", info),
            Self::Decode => write!(f, "failed to decode a key from a table"),
            Self::Stats(info) => write!(f, "failed to get stats: {}", info),
            Self::LogLevelUnavailable(level) => write!(f, "log level {:?} is not available", level),
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}

/// Common error struct to propagate implementation-specific error information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseErrorInfo {
    /// Human-readable error message.
    pub message: String,
    /// Error code.
    pub code: i32,
}

#[cfg(feature = "std")]
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

#[cfg(feature = "std")]
impl std::error::Error for DatabaseErrorInfo {}

impl Display for DatabaseErrorInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_fmt(format_args!("{0} ({1})", self.message, self.code))
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

#[cfg(feature = "std")]
impl std::error::Error for DatabaseWriteError {}

impl Display for DatabaseWriteError {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!(
            "write operation {0:?} failed for key \"{1}\" in table {2:?}: {3}",
            self.operation,
            reth_primitives::hex::encode(&self.key),
            self.table_name,
            self.info,
        ))
    }
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

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "fatal" => Ok(Self::Fatal),
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "notice" => Ok(Self::Notice),
            "verbose" => Ok(Self::Verbose),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            "extra" => Ok(Self::Extra),
            _ => Err(format!("Invalid log level: {}", s)),
        }
    }
}
