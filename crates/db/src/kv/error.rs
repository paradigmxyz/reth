//! KVError declaration.

use libmdbx::Error;
use thiserror::Error;

/// KV error type.
#[derive(Debug, Error)]
pub enum KVError {
    /// Generic MDBX error.
    #[error("MDBX error: {0:?}")]
    MDBX(#[from] Error),
    /// Failed to open MDBX file.
    #[error("{0:?}")]
    DatabaseLocation(Error),
    /// Failed to create a table in database.
    #[error("{0:?}")]
    TableCreation(Error),
    /// Failed to insert a value into a table.
    #[error("{0:?}")]
    Put(Error),
    /// Failed to get a value into a table.
    #[error("{0:?}")]
    Get(Error),
    /// Failed to delete a `(key, vakue)` pair into a table.
    #[error("{0:?}")]
    Delete(Error),
    /// Failed to commit transaction changes into the database.
    #[error("{0:?}")]
    Commit(Error),
    /// Failed to initiate a MDBX transaction.
    #[error("{0:?}")]
    InitTransaction(Error),
    /// Failed to decode or encode a key or value coming from a table..
    #[error("{0:?}")]
    InvalidValue(Option<String>),
}
