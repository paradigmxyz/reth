use thiserror::Error;

/// Database error type.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum DatabaseError {
    /// Failed to open the database.
    #[error("failed to open the database ({0})")]
    Open(i32),
    /// Failed to create a table in the database.
    #[error("failed to create a table ({0})")]
    CreateTable(i32),
    /// Failed to write a value into a table.
    #[error(transparent)]
    Write(#[from] Box<DatabaseWriteError>),
    /// Failed to read a value from a table.
    #[error("failed to read a value from a database table ({0})")]
    Read(i32),
    /// Failed to delete a `(key, value)` pair from a table.
    #[error("database delete error code ({0})")]
    Delete(i32),
    /// Failed to commit transaction changes into the database.
    #[error("failed to commit transaction changes ({0})")]
    Commit(i32),
    /// Failed to initiate a transaction.
    #[error("failed to initialize a transaction ({0})")]
    InitTx(i32),
    /// Failed to initialize a cursor.
    #[error("failed to initialize a cursor ({0})")]
    InitCursor(i32),
    /// Failed to decode a key from a table.
    #[error("failed to decode a key from a table")]
    Decode,
    /// Failed to get database stats.
    #[error("failed to get stats ({0})")]
    Stats(i32),
    /// Failed to use the specified log level, as it's not available.
    #[error("log level {0:?} is not available")]
    LogLevelUnavailable(LogLevel),
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
    "write operation {operation:?} failed for key \"{key}\" in table {table_name:?} ({code})",
    key = reth_primitives::hex::encode(key),
)]
pub struct DatabaseWriteError {
    /// The error code.
    pub code: i32,
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
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
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
