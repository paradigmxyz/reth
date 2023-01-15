/// Database error type. They are using u32 to represent error code.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum Error {
    /// Failed to open database.
    #[error("{0:?}")]
    DatabaseLocation(u32),
    /// Failed to create a table in database.
    #[error("Table Creating error code: {0:?}")]
    TableCreation(u32),
    /// Failed to insert a value into a table.
    #[error("Database write error code: {0:?}")]
    Write(u32),
    /// Failed to get a value into a table.
    #[error("Database read error code: {0:?}")]
    Read(u32),
    /// Failed to delete a `(key, value)` pair into a table.
    #[error("Database delete error code: {0:?}")]
    Delete(u32),
    /// Failed to commit transaction changes into the database.
    #[error("Database commit error code: {0:?}")]
    Commit(u32),
    /// Failed to initiate a transaction.
    #[error("Initialization of transaction errored with code: {0:?}")]
    InitTransaction(u32),
    /// Failed to initiate a cursor.
    #[error("Initialization of cursor errored with code: {0:?}")]
    InitCursor(u32),
    /// Failed to decode a key from a table..
    #[error("Error decoding value.")]
    DecodeError,
}
