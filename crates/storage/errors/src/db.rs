use clap::{
    builder::{PossibleValue, TypedValueParser},
    error::ErrorKind,
    Arg, Command, Error,
};
use std::{fmt::Display, str::FromStr};
use thiserror::Error;

/// Database error type.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
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
}

/// Common error struct to propagate implementation-specific error information.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq, Error)]
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
    /// All possible variants of the LogLevel enum
    pub fn value_variants() -> &'static [LogLevel] {
        &[
            LogLevel::Fatal,
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Notice,
            LogLevel::Verbose,
            LogLevel::Debug,
            LogLevel::Trace,
            LogLevel::Extra,
        ]
    }

    /// Returns all variants descriptions
    pub fn help_message(&self) -> &'static str {
        match self {
            LogLevel::Fatal => "Enables logging for critical conditions, i.e. assertion failures",
            LogLevel::Error => "Enables logging for error conditions",
            LogLevel::Warn => "Enables logging for warning conditions",
            LogLevel::Notice => "Enables logging for normal but significant conditions",
            LogLevel::Verbose => "Enables logging for verbose informational",
            LogLevel::Debug => "Enables logging for debug-level messages",
            LogLevel::Trace => "Enables logging for trace debug-level messages",
            LogLevel::Extra => "Enables logging for extra debug-level messages",
        }
    }
}

impl FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "fatal" => Ok(LogLevel::Fatal),
            "error" => Ok(LogLevel::Error),
            "warn" => Ok(LogLevel::Warn),
            "notice" => Ok(LogLevel::Notice),
            "verbose" => Ok(LogLevel::Verbose),
            "debug" => Ok(LogLevel::Debug),
            "trace" => Ok(LogLevel::Trace),
            "extra" => Ok(LogLevel::Extra),
            _ => Err(format!("Invalid log level: {}", s)),
        }
    }
}

/// clap value parser for [LogLevel].
#[derive(Clone, Debug, Default)]
pub struct LogLevelValueParser;

impl TypedValueParser for LogLevelValueParser {
    type Value = LogLevel;

    fn parse_ref(
        &self,
        _cmd: &Command,
        arg: Option<&Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, Error> {
        let val =
            value.to_str().ok_or_else(|| Error::raw(ErrorKind::InvalidUtf8, "Invalid UTF-8"))?;

        val.parse::<LogLevel>().map_err(|err| {
            let arg = arg.map(|a| a.to_string()).unwrap_or_else(|| "...".to_owned());
            let possible_values = LogLevel::value_variants()
                .iter()
                .map(|v| format!("{:?}", v))
                .collect::<Vec<_>>()
                .join(", ");
            let msg = format!(
                "Invalid value '{val}' for {arg}: {err}.\n    [possible values: {possible_values}]"
            );
            clap::Error::raw(clap::error::ErrorKind::InvalidValue, msg)
        })
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        let values =
            LogLevel::value_variants().iter().map(|v| PossibleValue::new(v.help_message()));
        Some(Box::new(values))
    }
}
