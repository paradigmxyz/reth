/// Database error type. They are using u32 to represent error code.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum DatabaseError {
    /// Failed to open database.
    #[error("Failed to open database: {0:?}")]
    FailedToOpen(i32),
    /// Failed to create a table in database.
    #[error("Table Creating error code: {0:?}")]
    TableCreation(i32),
    /// Failed to insert a value into a table.
    #[error("Database write error code: {0:?}")]
    Write(i32),
    /// Failed to get a value into a table.
    #[error("Database read error code: {0:?}")]
    Read(i32),
    /// Failed to delete a `(key, value)` pair into a table.
    #[error("Database delete error code: {0:?}")]
    Delete(i32),
    /// Failed to commit transaction changes into the database.
    #[error("Database commit error code: {0:?}")]
    Commit(i32),
    /// Failed to initiate a transaction.
    #[error("Initialization of transaction errored with code: {0:?}")]
    InitTransaction(i32),
    /// Failed to initiate a cursor.
    #[error("Initialization of cursor errored with code: {0:?}")]
    InitCursor(i32),
    /// Failed to decode a key from a table.
    #[error("Error decoding value.")]
    DecodeError,
    /// Failed to get database stats.
    #[error("Database stats error code: {0:?}")]
    Stats(i32),
    /// Failed to use the specified log level, as it's not available.
    #[error("Log level is not available: {0:?}")]
    LogLevelUnavailable(LogLevel),
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
/// Database log level.
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
