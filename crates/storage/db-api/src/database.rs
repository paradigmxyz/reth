use crate::{
    table::TableImporter,
    transaction::{DbTx, DbTxMut, DbTxMutUnsync, DbTxUnsync},
    DatabaseError,
};
use std::{fmt::Debug, sync::Arc};

/// Main Database trait that can open read-only and read-write transactions.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for consumption.
pub trait Database: Send + Sync + Debug {
    /// Read-Only database transaction (thread-safe, synchronized)
    type TX: DbTx + Send + Sync + Debug + 'static;
    /// Read-Write database transaction (thread-safe, synchronized)
    type TXMut: DbTxMut + DbTx + TableImporter + Send + Sync + Debug + 'static;

    /// Read-Only database transaction (unsynchronized, faster for single-threaded use)
    type TXUnsync: DbTxUnsync + Debug + 'static;
    /// Read-Write database transaction (unsynchronized, faster for single-threaded use)
    type TXMutUnsync: DbTxMutUnsync + DbTxUnsync + Debug + 'static;

    /// Create read only transaction (thread-safe, synchronized).
    #[track_caller]
    fn tx(&self) -> Result<Self::TX, DatabaseError>;

    /// Create read write transaction only possible if database is open with write access
    /// (thread-safe, synchronized).
    #[track_caller]
    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError>;

    /// Create read only transaction (unsynchronized, faster for single-threaded use).
    ///
    /// This variant skips mutex synchronization and is more efficient when you know
    /// the transaction will only be used from a single thread.
    #[track_caller]
    fn tx_unsync(&self) -> Result<Self::TXUnsync, DatabaseError>;

    /// Create read write transaction (unsynchronized, faster for single-threaded use).
    ///
    /// This variant skips mutex synchronization and is more efficient when you know
    /// the transaction will only be used from a single thread.
    #[track_caller]
    fn tx_mut_unsync(&self) -> Result<Self::TXMutUnsync, DatabaseError>;

    /// Takes a function and passes a read-only transaction into it, making sure it's closed in the
    /// end of the execution.
    fn view<T, F>(&self, f: F) -> Result<T, DatabaseError>
    where
        F: FnOnce(&mut Self::TX) -> T,
    {
        let mut tx = self.tx()?;

        let res = f(&mut tx);
        tx.commit()?;

        Ok(res)
    }

    /// Takes a function and passes a write-read transaction into it, making sure it's committed in
    /// the end of the execution.
    fn update<T, F>(&self, f: F) -> Result<T, DatabaseError>
    where
        F: FnOnce(&Self::TXMut) -> T,
    {
        let tx = self.tx_mut()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }
}

impl<DB: Database> Database for Arc<DB> {
    type TX = <DB as Database>::TX;
    type TXMut = <DB as Database>::TXMut;
    type TXUnsync = <DB as Database>::TXUnsync;
    type TXMutUnsync = <DB as Database>::TXMutUnsync;

    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        <DB as Database>::tx(self)
    }

    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        <DB as Database>::tx_mut(self)
    }

    fn tx_unsync(&self) -> Result<Self::TXUnsync, DatabaseError> {
        <DB as Database>::tx_unsync(self)
    }

    fn tx_mut_unsync(&self) -> Result<Self::TXMutUnsync, DatabaseError> {
        <DB as Database>::tx_mut_unsync(self)
    }
}

impl<DB: Database> Database for &DB {
    type TX = <DB as Database>::TX;
    type TXMut = <DB as Database>::TXMut;
    type TXUnsync = <DB as Database>::TXUnsync;
    type TXMutUnsync = <DB as Database>::TXMutUnsync;

    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        <DB as Database>::tx(self)
    }

    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        <DB as Database>::tx_mut(self)
    }

    fn tx_unsync(&self) -> Result<Self::TXUnsync, DatabaseError> {
        <DB as Database>::tx_unsync(self)
    }

    fn tx_mut_unsync(&self) -> Result<Self::TXMutUnsync, DatabaseError> {
        <DB as Database>::tx_mut_unsync(self)
    }
}
