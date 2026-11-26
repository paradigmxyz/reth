use crate::{table::TableImporter, transaction::DbTx, DatabaseError};
use std::{
    fmt::Debug,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{error, warn};

/// How often [`ReaderTxnTracker::wait_for_pre_commit_readers`] clears reader slots of dead
/// processes.
const READER_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// How often [`ReaderTxnTracker::wait_for_pre_commit_readers`] warns while still waiting.
const SLOW_READERS_WARN_INTERVAL: Duration = Duration::from_secs(10);

/// Main Database trait that can open read-only and read-write transactions.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for consumption.
pub trait Database: Send + Sync + Debug {
    /// Read-Only database transaction
    type TX: DbTx + Sync + 'static;
    /// Read-Write database transaction
    type TXMut: DbTx + TableImporter + Sync + 'static;

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

    /// Clears reader slots left behind by processes that no longer exist, so they stop counting
    /// towards [`Self::oldest_reader_txnid`].
    ///
    /// MDBX skips slots carrying this process's own PID, so a stale slot left by a dead process
    /// whose PID was reused by this one is not cleared.
    ///
    /// Does nothing by default, for backends without such slots.
    fn reader_check(&self) -> Result<(), DatabaseError> {
        Ok(())
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

    fn reader_check(&self) -> Result<(), DatabaseError> {
        <DB as Database>::reader_check(self)
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

    fn reader_check(&self) -> Result<(), DatabaseError> {
        <DB as Database>::reader_check(self)
    }
}

/// Object-safe adapter for reader-txn tracking during unwind.
pub trait ReaderTxnTracker: Send + Sync {
    /// Waits until all readers older than the latest committed txnid have drained.
    ///
    /// Returns an error only if the environment stops answering queries, such as after a fatal
    /// MDBX error, since then it can no longer tell whether old readers remain.
    fn wait_for_pre_commit_readers(&self) -> Result<(), DatabaseError>;
}

impl<DB: Database> ReaderTxnTracker for DB {
    /// There is intentionally no deadline: proceeding while a reader still sees the pre-commit
    /// view would let it observe a mix of old MDBX state and later cross-store unwind steps.
    /// Instead, slots of dead processes are periodically cleared and a slow wait is logged.
    fn wait_for_pre_commit_readers(&self) -> Result<(), DatabaseError> {
        // A committed txnid is never 0, so `None` here means the environment stopped answering.
        let Some(committed_txnid) = Database::last_txnid(self) else {
            return Database::reader_check(self)
        };
        let start = Instant::now();
        let mut last_reader_check = start;
        let mut last_warn = start;
        loop {
            let oldest_reader_txnid = Database::oldest_reader_txnid(self);
            match oldest_reader_txnid {
                Some(oldest) if oldest >= committed_txnid => return Ok(()),
                // `None` means no readers only while the environment still answers queries. If
                // it stopped answering, as after a fatal MDBX error, surface the reader check's
                // error instead of releasing readers that may still see the pre-commit view.
                None if Database::last_txnid(self).is_some() => return Ok(()),
                None => Database::reader_check(self)?,
                _ => {}
            }
            if last_reader_check.elapsed() >= READER_CHECK_INTERVAL {
                if let Err(err) = Database::reader_check(self) {
                    error!(target: "storage::db", %err, "Failed to clear stale database readers");
                }
                last_reader_check = Instant::now();
                // Re-sample before warning, since the check may have cleared the oldest reader.
                continue
            }
            if last_warn.elapsed() >= SLOW_READERS_WARN_INTERVAL {
                warn!(
                    target: "storage::db",
                    ?oldest_reader_txnid,
                    committed_txnid,
                    elapsed = ?start.elapsed(),
                    "Waiting for database readers older than the last commit to finish"
                );
                last_warn = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::TxMock;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    /// Mimics a fatal MDBX environment: after the commit, every environment query fails.
    #[derive(Debug, Default)]
    struct FatalAfterCommitDb {
        /// Whether the first `last_txnid` query still succeeds.
        answers_first_query: bool,
        queried: AtomicBool,
    }

    impl Database for FatalAfterCommitDb {
        type TX = TxMock;
        type TXMut = TxMock;

        fn tx(&self) -> Result<Self::TX, DatabaseError> {
            Ok(TxMock::default())
        }

        fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
            Ok(TxMock::default())
        }

        fn path(&self) -> PathBuf {
            PathBuf::default()
        }

        fn oldest_reader_txnid(&self) -> Option<u64> {
            None
        }

        fn last_txnid(&self) -> Option<u64> {
            (self.answers_first_query && !self.queried.swap(true, Ordering::SeqCst)).then_some(2)
        }

        fn reader_check(&self) -> Result<(), DatabaseError> {
            Err(DatabaseError::Other("fatal".into()))
        }
    }

    #[test]
    fn wait_for_pre_commit_readers_fails_on_fatal_env() {
        // The environment turns fatal either during the wait or before it starts.
        for answers_first_query in [true, false] {
            let db = FatalAfterCommitDb { answers_first_query, ..Default::default() };
            let (done_tx, done_rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = done_tx.send(db.wait_for_pre_commit_readers());
            });
            let result = done_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("wait_for_pre_commit_readers hung on a fatal environment");
            assert!(
                matches!(&result, Err(DatabaseError::Other(msg)) if msg == "fatal"),
                "answers_first_query={answers_first_query}: {result:?}"
            );
        }
    }
}
