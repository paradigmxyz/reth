use crate::{
    table::TableImporter,
    tables::{self, RawKey, RawTable, RawValue},
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
    fn oldest_reader_txnid(&self) -> Option<u64> {
        None
    }

    /// Returns the ID of the most recently committed transaction, if available.
    fn last_txnid(&self) -> Option<u64> {
        None
    }

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

/// Object-safe adapter for reader-txn tracking and unwind fencing.
pub trait ReaderTxnTracker: Send + Sync {
    /// Returns the txnid of the oldest active reader snapshot.
    ///
    /// This is not a unique per-reader id; it is the committed txnid the reader is pinned to.
    fn oldest_reader_txnid(&self) -> Option<u64>;

    /// Returns the latest committed MDBX txnid.
    fn last_txnid(&self) -> Option<u64>;

    /// Forces a real commit so the latest txnid advances, then returns it.
    fn commit_fence(&self) -> Result<Option<u64>, DatabaseError>;

    /// Waits until all readers pinned to a committed txnid older than `cutoff_txnid` have drained,
    /// polling every 10ms.
    fn wait_for_readers_before_txnid(&self, cutoff_txnid: u64) {
        while self.oldest_reader_txnid().is_some_and(|oldest| oldest < cutoff_txnid) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl<DB: Database> ReaderTxnTracker for DB {
    fn oldest_reader_txnid(&self) -> Option<u64> {
        Database::oldest_reader_txnid(self)
    }

    fn last_txnid(&self) -> Option<u64> {
        Database::last_txnid(self)
    }

    fn commit_fence(&self) -> Result<Option<u64>, DatabaseError> {
        let last_txnid = self.last_txnid().unwrap_or_default();
        let tx = self.tx_mut()?;
        tx.put::<RawTable<tables::Metadata>>(
            RawKey::<String>::from_vec(vec![0, 1]),
            RawValue::from_vec(last_txnid.to_be_bytes().into()),
        )?;
        tx.commit()?;
        Ok(self.last_txnid())
    }
}
