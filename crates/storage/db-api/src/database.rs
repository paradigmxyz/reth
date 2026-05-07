use crate::{
    table::TableImporter,
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use std::{fmt::Debug, path::PathBuf, sync::Arc};

/// Main Database trait that can open read-only and read-write transactions.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for consumption.
pub trait Database: Send + Sync + Debug {
    /// Read-Only database transaction
    type TX: DbTx + Send + Sync + Debug + 'static;
    /// Read-Write database transaction
    type TXMut: DbTxMut + DbTx + TableImporter + Send + Sync + Debug + 'static;

    /// Create read only transaction.
    #[track_caller]
    fn tx(&self) -> Result<Self::TX, DatabaseError>;

    /// Create read write transaction only possible if database is open with write access.
    #[track_caller]
    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError>;

    /// Returns the path to the database directory.
    fn path(&self) -> PathBuf;

    /// Returns the transaction ID of the oldest active reader, if available.
    ///
    /// This is the committed txnid of the snapshot the reader is pinned to, not a unique per-reader
    /// identifier, so multiple readers can report the same txnid.
    ///
    /// Used to check whether stale readers from a previous write transaction have completed.
    /// Returns `None` if no readers are active or the backend does not support this query.
    fn oldest_reader_txnid(&self) -> Option<u64>;

    /// Returns the ID of the most recently committed transaction, if available.
    fn last_txnid(&self) -> Option<u64>;

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

    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        <DB as Database>::tx(self)
    }

    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        <DB as Database>::tx_mut(self)
    }

    fn path(&self) -> PathBuf {
        <DB as Database>::path(self)
    }

    fn oldest_reader_txnid(&self) -> Option<u64> {
        <DB as Database>::oldest_reader_txnid(self)
    }

    fn last_txnid(&self) -> Option<u64> {
        <DB as Database>::last_txnid(self)
    }
}

impl<DB: Database> Database for &DB {
    type TX = <DB as Database>::TX;
    type TXMut = <DB as Database>::TXMut;

    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        <DB as Database>::tx(self)
    }

    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        <DB as Database>::tx_mut(self)
    }

    fn path(&self) -> PathBuf {
        <DB as Database>::path(self)
    }

    fn oldest_reader_txnid(&self) -> Option<u64> {
        <DB as Database>::oldest_reader_txnid(self)
    }

    fn last_txnid(&self) -> Option<u64> {
        <DB as Database>::last_txnid(self)
    }
}

/// Object-safe adapter for reader-txn tracking during unwind.
pub trait ReaderTxnTracker: Send + Sync {
    /// Waits until all readers older than the latest committed txnid have drained.
    fn wait_for_pre_commit_readers(&self);
}

impl<DB: Database> ReaderTxnTracker for DB {
    fn wait_for_pre_commit_readers(&self) {
        if let Some(committed_txnid) = Database::last_txnid(self) {
            while Database::oldest_reader_txnid(self).is_some_and(|oldest| oldest < committed_txnid)
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}
