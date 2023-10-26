/// Database error type.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum DatabaseError {
    /// Failed to open the database.
    #[error("failed to open the database ({0})")]
    Open(i32),
    /// Failed to create a table in the database.
    #[error("failed to create a table ({0})")]
    CreateTable(i32),
    /// Failed to write a value into a table.
    #[error(
        "write operation {operation:?} failed for key \"{key}\" in table {table_name:?} ({code})",
        key = reth_primitives::hex::encode(key),
    )]
    Write {
        /// Database error code
        code: i32,
        /// Database write operation type
        operation: DatabaseWriteOperation,
        /// Table name
        table_name: &'static str,
        /// Write key
        key: Box<[u8]>,
    },
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

/// Database write operation type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum DatabaseWriteOperation {
    CursorAppend,
    CursorUpsert,
    CursorInsert,
    CursorAppendDup,
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
