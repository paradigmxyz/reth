use crate::{
    environment::EnvPtr,
    error::{mdbx_result, Result},
    CommitLatency,
};
use std::{
    fmt, ptr,
    sync::mpsc::{sync_channel, Receiver, SyncSender},
};

#[derive(Copy, Clone, Debug)]
pub(crate) struct TxnPtr(pub(crate) *mut ffi::MDBX_txn);
unsafe impl Send for TxnPtr {}
unsafe impl Sync for TxnPtr {}

pub(crate) enum TxnManagerMessage {
    Begin { parent: TxnPtr, flags: ffi::MDBX_txn_flags_t, sender: SyncSender<Result<TxnPtr>> },
    Abort { tx: TxnPtr, sender: SyncSender<Result<bool>> },
    Commit { tx: TxnPtr, sender: SyncSender<Result<(bool, CommitLatency)>> },
}

impl fmt::Debug for TxnManagerMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Begin { parent, flags, sender: _ } => {
                f.debug_struct("Begin").field("parent", parent).field("flags", flags).finish()
            }
            Self::Abort { tx, sender: _ } => f.debug_struct("Abort").field("tx", tx).finish(),
            Self::Commit { tx, sender: _ } => f.debug_struct("Commit").field("tx", tx).finish(),
        }
    }
}

/// Manages transactions by doing two things:
/// - Opening, aborting, and committing transactions using [`TxnManager::send_message`] with the
///   corresponding [`TxnManagerMessage`]
/// - Timing out long-lived read transactions (if the `read-tx-timeouts` feature is enabled and
///   `TxnManager::with_max_read_transaction_duration` is called)
#[derive(Debug)]
pub(crate) struct TxnManager {
    sender: SyncSender<TxnManagerMessage>,
    #[cfg(feature = "read-tx-timeouts")]
    read_transactions: Option<std::sync::Arc<read_transactions::ReadTransactions>>,
}

impl TxnManager {
    pub(crate) fn new(env: EnvPtr) -> Self {
        let (tx, rx) = sync_channel(0);
        let txn_manager = Self {
            sender: tx,
            #[cfg(feature = "read-tx-timeouts")]
            read_transactions: None,
        };

        txn_manager.start_message_listener(env, rx);

        txn_manager
    }

    /// Spawns a new [`std::thread`] that listens to incoming [`TxnManagerMessage`] messages,
    /// executes an FFI function, and returns the result on the provided channel.
    ///
    /// - [`TxnManagerMessage::Begin`] opens a new transaction with [`ffi::mdbx_txn_begin_ex`]
    /// - [`TxnManagerMessage::Abort`] aborts a transaction with [`ffi::mdbx_txn_abort`]
    /// - [`TxnManagerMessage::Commit`] commits a transaction with [`ffi::mdbx_txn_commit_ex`]
    fn start_message_listener(&self, env: EnvPtr, rx: Receiver<TxnManagerMessage>) {
        let task = move || {
            let env = env;
            loop {
                let msg = rx.recv();
                tracing::debug!(target: "libmdbx::txn", ?msg, "txn-mngr received");
                match msg {
                    Ok(msg) => match msg {
                        TxnManagerMessage::Begin { parent, flags, sender } => {
                            let _span =
                                tracing::debug_span!(target: "libmdbx::txn", "begin", flags)
                                    .entered();
                            let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
                            let res = mdbx_result(unsafe {
                                ffi::mdbx_txn_begin_ex(
                                    env.0,
                                    parent.0,
                                    flags,
                                    &mut txn,
                                    ptr::null_mut(),
                                )
                            })
                            .map(|_| TxnPtr(txn));
                            sender.send(res).unwrap();
                        }
                        TxnManagerMessage::Abort { tx, sender } => {
                            let _span =
                                tracing::debug_span!(target: "libmdbx::txn", "abort").entered();
                            sender.send(mdbx_result(unsafe { ffi::mdbx_txn_abort(tx.0) })).unwrap();
                        }
                        TxnManagerMessage::Commit { tx, sender } => {
                            let _span =
                                tracing::debug_span!(target: "libmdbx::txn", "commit").entered();
                            sender
                                .send({
                                    let mut latency = CommitLatency::new();
                                    mdbx_result(unsafe {
                                        ffi::mdbx_txn_commit_ex(tx.0, latency.mdbx_commit_latency())
                                    })
                                    .map(|v| (v, latency))
                                })
                                .unwrap();
                        }
                    },
                    Err(_) => return,
                }
            }
        };
        std::thread::Builder::new().name("mdbx-rs-txn-mgr".to_string()).spawn(task).unwrap();
    }

    pub(crate) fn send_message(&self, message: TxnManagerMessage) {
        self.sender.send(message).unwrap()
    }
}

#[cfg(feature = "read-tx-timeouts")]
mod read_transactions {
    use crate::{environment::EnvPtr, txn_manager::TxnManager};
    use dashmap::{DashMap, DashSet};
    use std::{
        backtrace::Backtrace,
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc::sync_channel,
            Arc,
        },
        time::{Duration, Instant},
    };
    use tracing::{trace, warn};

    const READ_TRANSACTIONS_CHECK_INTERVAL: Duration = Duration::from_secs(5);

    /// Timeout flag, opening time, and opening backtrace of an active read transaction.
    type ActiveReadTransaction = (Arc<AtomicBool>, Instant, Option<Arc<Backtrace>>);

    impl TxnManager {
        /// Returns a new instance for which the maximum duration that a read transaction can be
        /// open is set.
        pub(crate) fn new_with_max_read_transaction_duration(
            env: EnvPtr,
            duration: Duration,
        ) -> Self {
            let read_transactions = Arc::new(ReadTransactions::new(duration));
            read_transactions.clone().start_monitor();

            let (tx, rx) = sync_channel(0);

            let txn_manager = Self { sender: tx, read_transactions: Some(read_transactions) };

            txn_manager.start_message_listener(env, rx);

            txn_manager
        }

        /// Adds a new transaction to the list of active read transactions.
        ///
        /// Returns a flag that is set once the transaction has been open for too long, or `None` if
        /// read transactions are not monitored.
        pub(crate) fn add_active_read_transaction(
            &self,
            ptr: *mut ffi::MDBX_txn,
        ) -> Option<Arc<AtomicBool>> {
            self.read_transactions
                .as_ref()
                .map(|read_transactions| read_transactions.add_active(ptr))
        }

        /// Removes a transaction from the list of active read transactions.
        pub(crate) fn remove_active_read_transaction(&self, ptr: *mut ffi::MDBX_txn) {
            if let Some(read_transactions) = &self.read_transactions {
                read_transactions.remove_active(ptr);
            }
        }

        /// Stops timing out a read transaction. A transaction that has already timed out still
        /// holds its snapshot, so it stays in the list of timed out transactions until dropped.
        pub(crate) fn disable_read_transaction_timeout(&self, ptr: *mut ffi::MDBX_txn) {
            if let Some(read_transactions) = &self.read_transactions {
                read_transactions.active.remove(&(ptr as usize));
            }
        }

        /// Returns the number of timed out transactions that were not aborted by the user yet.
        pub(crate) fn timed_out_not_aborted_read_transactions(&self) -> Option<usize> {
            self.read_transactions
                .as_ref()
                .map(|read_transactions| read_transactions.timed_out_not_aborted())
        }
    }

    #[derive(Debug, Default)]
    pub(super) struct ReadTransactions {
        /// Maximum duration that a read transaction can be open until the
        /// [`ReadTransactions::start_monitor`] times it out.
        max_duration: Duration,
        /// List of currently active read transactions.
        ///
        /// We store `usize` instead of a raw pointer as a key, because pointers are not
        /// comparable. The timeout flag of the transaction and the time of transaction opening are
        /// stored as a value.
        ///
        /// The backtrace of the transaction opening is recorded only when debug assertions are
        /// enabled outside DST.
        active: DashMap<usize, ActiveReadTransaction>,
        /// List of timed out transactions that were not aborted by the user yet, hence still hold
        /// their MVCC snapshot.
        timed_out_not_aborted: DashSet<usize>,
    }

    impl ReadTransactions {
        pub(super) fn new(max_duration: Duration) -> Self {
            Self { max_duration, ..Default::default() }
        }

        /// Adds a new transaction to the list of active read transactions and returns its timeout
        /// flag.
        pub(super) fn add_active(&self, ptr: *mut ffi::MDBX_txn) -> Arc<AtomicBool> {
            let timed_out = Arc::new(AtomicBool::new(false));
            let _ = self.active.insert(
                ptr as usize,
                (
                    timed_out.clone(),
                    Instant::now(),
                    cfg!(all(debug_assertions, not(feature = "dst")))
                        .then(|| Arc::new(Backtrace::force_capture())),
                ),
            );
            timed_out
        }

        /// Removes a transaction from the list of active read transactions.
        pub(super) fn remove_active(&self, ptr: *mut ffi::MDBX_txn) {
            // Remove from `active` first: the monitor moves an entry to `timed_out_not_aborted`
            // while holding the `active` shard lock, so the entry can't be moved after this.
            self.active.remove(&(ptr as usize));
            self.timed_out_not_aborted.remove(&(ptr as usize));
        }

        /// Returns the number of timed out transactions that were not aborted by the user yet.
        pub(super) fn timed_out_not_aborted(&self) -> usize {
            self.timed_out_not_aborted.len()
        }

        /// Spawns a new [`std::thread`] that monitors the list of active read transactions and
        /// timeouts those that are open for longer than `ReadTransactions.max_duration`.
        pub(super) fn start_monitor(self: Arc<Self>) {
            let task = move || {
                loop {
                    let now = Instant::now();
                    let mut max_active_transaction_duration = None;
                    let mut newly_timed_out = Vec::new();

                    // Time out and remove read transactions that are open for longer than
                    // `self.max_duration`.
                    self.active.retain(|&ptr, (timed_out, start, backtrace)| {
                        let duration = now - *start;
                        if duration <= self.max_duration {
                            max_active_transaction_duration = Some(
                                duration.max(max_active_transaction_duration.unwrap_or_default()),
                            );
                            return true
                        }

                        // Only the thread that owns the transaction may use it, so it is not reset
                        // here. Every following operation on it fails, and it releases its
                        // snapshot when dropped.
                        timed_out.store(true, Ordering::Relaxed);
                        self.timed_out_not_aborted.insert(ptr);
                        newly_timed_out.push((duration, backtrace.take()));
                        false
                    });

                    // Log outside of `retain`, which holds a shard lock that blocks opening and
                    // dropping read transactions.
                    for (open_duration, backtrace) in newly_timed_out {
                        warn!(target: "libmdbx", ?open_duration, ?backtrace, "Long-lived read transaction has been timed out");
                    }

                    if !self.active.is_empty() {
                        trace!(
                            target: "libmdbx",
                            elapsed = ?now.elapsed(),
                            active = ?self.active.iter().map(|entry| {
                                let (_, start, _) = entry.value();
                                (*entry.key() as *const ffi::MDBX_txn, start.elapsed())
                            }).collect::<Vec<_>>(),
                            "Read transactions"
                        );
                    }

                    // Sleep not more than `READ_TRANSACTIONS_CHECK_INTERVAL`, but at least until
                    // the closest deadline of an active read transaction
                    let sleep_duration = READ_TRANSACTIONS_CHECK_INTERVAL.min(
                        self.max_duration - max_active_transaction_duration.unwrap_or_default(),
                    );
                    trace!(target: "libmdbx", ?sleep_duration, elapsed = ?now.elapsed(), "Putting transaction monitor to sleep");
                    std::thread::sleep(sleep_duration);
                }
            };
            std::thread::Builder::new()
                .name("mdbx-rs-read-tx-timeouts".to_string())
                .spawn(task)
                .unwrap();
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::{
            txn_manager::read_transactions::READ_TRANSACTIONS_CHECK_INTERVAL, Environment, Error,
            MaxReadTransactionDuration,
        };
        use std::{
            thread::sleep,
            time::{Duration, Instant},
        };
        use tempfile::tempdir;

        #[test]
        fn txn_manager_read_transactions_duration_set() {
            const MAX_DURATION: Duration = Duration::from_secs(1);

            let dir = tempdir().unwrap();
            let env = Environment::builder()
                .set_max_read_transaction_duration(MaxReadTransactionDuration::Set(MAX_DURATION))
                .open(dir.path())
                .unwrap();

            let read_transactions = env.txn_manager().read_transactions.as_ref().unwrap();

            // Create a read-only transaction, successfully use it, close it by dropping.
            {
                let tx = env.begin_ro_txn().unwrap();
                let tx_ptr = tx.txn() as usize;
                assert!(read_transactions.active.contains_key(&tx_ptr));
                assert_eq!(
                    read_transactions.active.get(&tx_ptr).unwrap().2.is_some(),
                    cfg!(all(debug_assertions, not(feature = "dst"))),
                );

                tx.open_db(None).unwrap();
                drop(tx);

                assert!(!read_transactions.active.contains_key(&tx_ptr));
            }

            // Create a read-only transaction, successfully use it, close it by committing.
            {
                let tx = env.begin_ro_txn().unwrap();
                let tx_ptr = tx.txn() as usize;
                assert!(read_transactions.active.contains_key(&tx_ptr));

                tx.open_db(None).unwrap();
                tx.commit().unwrap();

                assert!(!read_transactions.active.contains_key(&tx_ptr));
            }

            {
                // Create a read-only transaction with a cursor and observe it's in the list of
                // active transactions.
                let tx = env.begin_ro_txn().unwrap();
                let tx_ptr = tx.txn() as usize;
                assert!(read_transactions.active.contains_key(&tx_ptr));
                let mut cursor = tx.cursor(tx.open_db(None).unwrap().dbi()).unwrap();

                // Wait until the transaction is timed out by the manager.
                let deadline = Instant::now() + MAX_DURATION + READ_TRANSACTIONS_CHECK_INTERVAL;
                while !read_transactions.timed_out_not_aborted.contains(&tx_ptr) {
                    assert!(Instant::now() < deadline, "transaction was not timed out");
                    sleep(Duration::from_millis(10));
                }

                // Ensure that the transaction is not in the list of active transactions anymore,
                // and is in the list of timed out but not aborted transactions.
                assert!(!read_transactions.active.contains_key(&tx_ptr));
                assert!(read_transactions.timed_out_not_aborted.contains(&tx_ptr));

                // Use the timed out transaction and observe the `Error::ReadTransactionTimeout`
                assert_eq!(tx.open_db(None).err(), Some(Error::ReadTransactionTimeout));
                assert!(!read_transactions.active.contains_key(&tx_ptr));
                assert!(read_transactions.timed_out_not_aborted.contains(&tx_ptr));

                assert_eq!(tx.id().err(), Some(Error::ReadTransactionTimeout));
                assert!(!read_transactions.active.contains_key(&tx_ptr));
                assert!(read_transactions.timed_out_not_aborted.contains(&tx_ptr));

                assert_eq!(cursor.first::<(), ()>().err(), Some(Error::ReadTransactionTimeout));
                drop(cursor);

                // Disabling the timeout of a timed out transaction doesn't hide that it still
                // holds its snapshot.
                tx.disable_timeout();
                assert!(read_transactions.timed_out_not_aborted.contains(&tx_ptr));

                // Ensure that the transaction pointer is not reused when opening a new read-only
                // transaction.
                let new_tx = env.begin_ro_txn().unwrap();
                let new_tx_ptr = new_tx.txn() as usize;
                assert!(read_transactions.active.contains_key(&new_tx_ptr));
                assert_ne!(tx_ptr, new_tx_ptr);

                // Drop the transaction and ensure that it's not in the list of timed out but not
                // aborted transactions anymore.
                drop(tx);
                assert!(!read_transactions.timed_out_not_aborted.contains(&tx_ptr));
            }
        }

        #[test]
        fn txn_manager_read_transactions_duration_unbounded() {
            let dir = tempdir().unwrap();
            let env = Environment::builder()
                .set_max_read_transaction_duration(MaxReadTransactionDuration::Unbounded)
                .open(dir.path())
                .unwrap();

            assert!(env.txn_manager().read_transactions.is_none());

            let tx = env.begin_ro_txn().unwrap();
            assert!(tx.commit().is_ok())
        }
    }
}
