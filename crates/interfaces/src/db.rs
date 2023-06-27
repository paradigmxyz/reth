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
    #[error(transparent)]
    InitTransaction(InitTransactionError),
    /// Failed to initiate a cursor.
    #[error("Initialization of cursor errored with code: {0:?}")]
    InitCursor(i32),
    /// Failed to decode a key from a table.
    #[error("Error decoding value.")]
    DecodeError,
    /// Failed to get database stats.
    #[error("Database stats error code: {0:?}")]
    Stats(i32),
    /// Max database transactions reached.
    ///
    /// This is thrown when an mdbx transaction fails to open because the lock table already has the maximum number of transactions open.
    #[error("Too many active database readers: {0:?}")]
    MaxReaders(i32),
}

/// Error variants that can happen when initializing a transaction.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum InitTransactionError {
    /// Max database transactions reached.
    ///
    /// This is thrown when an mdbx transaction fails to open because the lock table already has the maximum number of transactions open.
    #[error("Too many active database readers")]
    MaxReaders,
    /// Any other error that can happen when initializing a transaction.
    #[error("Initialization of transaction errored with code: {0:?}")]
    Other(i32)
}

impl From<i32> for InitTransactionError {
    fn from(value: i32) -> Self {
        todo!()
    }
}