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
/// - Aborting long-lived read transactions (if the `read-tx-timeouts` feature is enabled and
///   `TxnManager::with_max_read_transaction_duration` is called)
#[derive(Debug)]
pub(crate) struct TxnManager {
    sender: SyncSender<TxnManagerMessage>,
    #[cfg(feature = "read-tx-timeouts")]
    read_transactions: Option<read_transactions::ReadTransactionMonitor>,
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
    use crate::{
        environment::EnvPtr, error::mdbx_result, transaction::TransactionPtr,
        txn_manager::TxnManager,
    };
    use dashmap::{DashMap, DashSet};
    use std::{
        backtrace::Backtrace,
        ops::Deref,
        sync::{
            mpsc::{channel, sync_channel, RecvTimeoutError, Sender},
            Arc,
        },
        thread::JoinHandle,
        time::{Duration, Instant},
    };
    use tracing::{error, trace, warn};

    const READ_TRANSACTIONS_CHECK_INTERVAL: Duration = Duration::from_secs(5);

    impl TxnManager {
        /// Returns a new instance for which the maximum duration that a read transaction can be
        /// open is set.
        pub(crate) fn new_with_max_read_transaction_duration(
            env: EnvPtr,
            duration: Duration,
        ) -> Self {
            let read_transactions = Arc::new(ReadTransactions::new(duration));
            let read_transactions = read_transactions.start_monitor();

            let (tx, rx) = sync_channel(0);

            let txn_manager = Self { sender: tx, read_transactions: Some(read_transactions) };

            txn_manager.start_message_listener(env, rx);

            txn_manager
        }

        /// Stops and joins the read transaction monitor before the environment is closed.
        pub(crate) fn stop_read_transaction_monitor(&mut self) {
            drop(self.read_transactions.take());
        }

        /// Adds a new transaction to the list of active read transactions.
        pub(crate) fn add_active_read_transaction(
            &self,
            ptr: *mut ffi::MDBX_txn,
            tx: TransactionPtr,
        ) {
            if let Some(read_transactions) = &self.read_transactions {
                read_transactions.add_active(ptr, tx);
            }
        }

        /// Removes a transaction from the list of active read transactions.
        ///
        /// Returns `true` if the transaction was found and removed.
        pub(crate) fn remove_active_read_transaction(&self, ptr: *mut ffi::MDBX_txn) -> bool {
            self.read_transactions.as_ref().is_some_and(|txs| txs.remove_active(ptr))
        }

        /// Returns the number of timed out transactions that were not aborted by the user yet.
        pub(crate) fn timed_out_not_aborted_read_transactions(&self) -> Option<usize> {
            self.read_transactions
                .as_ref()
                .map(|read_transactions| read_transactions.timed_out_not_aborted())
        }
    }

    /// Owns the monitor thread and joins it when the environment is closed.
    #[derive(Debug)]
    pub(super) struct ReadTransactionMonitor {
        transactions: Arc<ReadTransactions>,
        shutdown: Option<Sender<()>>,
        handle: Option<JoinHandle<()>>,
    }

    impl Deref for ReadTransactionMonitor {
        type Target = ReadTransactions;

        fn deref(&self) -> &Self::Target {
            &self.transactions
        }
    }

    impl Drop for ReadTransactionMonitor {
        fn drop(&mut self) {
            // Disconnecting wakes the monitor even if it has not started waiting yet.
            drop(self.shutdown.take());
            if let Some(handle) = self.handle.take() &&
                handle.join().is_err()
            {
                error!(target: "libmdbx", "Read transaction monitor panicked during shutdown");
            }
        }
    }

    #[derive(Debug, Default)]
    pub(super) struct ReadTransactions {
        /// Maximum duration that a read transaction can be open until the
        /// [`ReadTransactions::start_monitor`] aborts it.
        max_duration: Duration,
        /// List of currently active read transactions.
        ///
        /// We store `usize` instead of a raw pointer as a key, because pointers are not
        /// comparable. The time of transaction opening is stored as a value.
        ///
        /// The backtrace of the transaction opening is recorded only when debug assertions are
        /// enabled outside DST.
        active: DashMap<usize, (TransactionPtr, Instant, Option<Arc<Backtrace>>)>,
        /// List of timed out transactions that were not aborted by the user yet, hence have a
        /// dangling read transaction pointer.
        timed_out_not_aborted: DashSet<usize>,
    }

    impl ReadTransactions {
        pub(super) fn new(max_duration: Duration) -> Self {
            Self { max_duration, ..Default::default() }
        }

        /// Adds a new transaction to the list of active read transactions.
        pub(super) fn add_active(&self, ptr: *mut ffi::MDBX_txn, tx: TransactionPtr) {
            let _ = self.active.insert(
                ptr as usize,
                (
                    tx,
                    Instant::now(),
                    cfg!(all(debug_assertions, not(feature = "fuzz")))
                        .then(|| Arc::new(Backtrace::force_capture())),
                ),
            );
        }

        /// Removes a transaction from the list of active read transactions.
        pub(super) fn remove_active(&self, ptr: *mut ffi::MDBX_txn) -> bool {
            self.timed_out_not_aborted.remove(&(ptr as usize));
            self.active.remove(&(ptr as usize)).is_some()
        }

        /// Returns `true` if `tx` is still the active read transaction registered for `ptr`.
        ///
        /// Pointers can be reused by new read transactions, so the transaction identity is
        /// compared as well.
        fn is_active(&self, ptr: usize, tx: &TransactionPtr) -> bool {
            self.active.get(&ptr).is_some_and(|entry| entry.value().0.is_same(tx))
        }

        /// Returns the number of timed out transactions that were not aborted by the user yet.
        pub(super) fn timed_out_not_aborted(&self) -> usize {
            self.timed_out_not_aborted.len()
        }

        /// Spawns a new [`std::thread`] that monitors the list of active read transactions and
        /// timeouts those that are open for longer than `ReadTransactions.max_duration`.
        pub(super) fn start_monitor(self: Arc<Self>) -> ReadTransactionMonitor {
            let transactions = self.clone();
            let (shutdown, receiver) = channel();
            let task = move || {
                let mut expired = Vec::new();

                loop {
                    let now = Instant::now();
                    let mut max_active_transaction_duration = None;

                    // Collect active read transactions that are open for longer than
                    // `self.max_duration`.
                    //
                    // The transaction locks must not be taken while iterating: commit and drop
                    // hold the transaction lock and then remove the entry from `active`, which
                    // needs the write lock of the shard that the iterator is holding.
                    for entry in &self.active {
                        let (tx, start, backtrace) = entry.value();
                        let duration = now - *start;

                        if duration > self.max_duration {
                            expired.push((*entry.key(), tx.clone(), duration, backtrace.clone()));
                        } else {
                            max_active_transaction_duration = Some(
                                duration.max(max_active_transaction_duration.unwrap_or_default()),
                            );
                        }
                    }

                    // Time out the collected transactions.
                    for (ptr, tx, open_duration, backtrace) in &expired {
                        let ptr = *ptr;
                        let result = tx.txn_execute_fail_on_timeout(|txn_ptr| {
                            // The transaction could have been committed or dropped after it was
                            // collected, and its pointer could even have been reused by a new
                            // read transaction from the pool. We hold the transaction lock, so
                            // it can't be finished from now on. If the entry is still ours, the
                            // transaction is live.
                            if !self.is_active(ptr, tx) {
                                return None
                            }

                            // Time out the transaction.
                            //
                            // We use `mdbx_txn_reset` instead of `mdbx_txn_abort` here to
                            // prevent MDBX from reusing the pointer of the aborted
                            // transaction for new read-only transactions. This is
                            // important because we store the pointer in the `active` list
                            // and assume that it is unique.
                            //
                            // See https://libmdbx.dqdkfa.ru/group__c__transactions.html#gae9f34737fe60b0ba538d5a09b6a25c8d for more info.
                            let result = mdbx_result(unsafe { ffi::mdbx_txn_reset(txn_ptr) });
                            if result.is_ok() {
                                tx.set_timed_out();
                            }

                            // Delete the transaction from the list of active transactions while
                            // still holding the transaction lock, so that it can't be dropped
                            // in between.
                            let was_in_active = self.remove_active(txn_ptr);
                            match result {
                                Ok(_) => {
                                    // Happy path, the transaction has been timed out by us with
                                    // no errors. Add it to the list of timed out transactions
                                    // that were not aborted by the user yet.
                                    self.timed_out_not_aborted.insert(txn_ptr as usize);
                                    warn!(target: "libmdbx", ?open_duration, ?backtrace, "Long-lived read transaction has been timed out");
                                }
                                Err(err) if was_in_active => {
                                    // If the transaction was in the list of active transactions,
                                    // then user didn't abort it and we failed to do so.
                                    error!(target: "libmdbx", %err, ?open_duration, ?backtrace, "Failed to time out the long-lived read transaction");
                                }
                                Err(_) => {}
                            }
                            Some(())
                        });

                        if let Err(err) = result {
                            error!(target: "libmdbx", %err, ?backtrace, "Failed to abort the long-lived read transaction")
                        }
                    }

                    // Clear the list, but not de-allocate the reserved capacity to save on
                    // further pushes.
                    expired.clear();

                    if !self.active.is_empty() {
                        trace!(
                            target: "libmdbx",
                            elapsed = ?now.elapsed(),
                            active = ?self.active.iter().map(|entry| {
                                let (tx, start, _) = entry.value();
                                (tx.clone(), start.elapsed())
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
                    match receiver.recv_timeout(sleep_duration) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            };
            let handle = std::thread::Builder::new()
                .name("mdbx-rs-read-tx-timeouts".to_string())
                .spawn(task)
                .unwrap();

            ReadTransactionMonitor { transactions, shutdown: Some(shutdown), handle: Some(handle) }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{Environment, Error, MaxReadTransactionDuration};
        use std::{
            sync::mpsc,
            thread::{self, sleep},
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
                    cfg!(all(debug_assertions, not(feature = "fuzz"))),
                );

                let snapshot = read_transactions.active.get(&tx_ptr).unwrap().0.clone();
                assert!(read_transactions.is_active(tx_ptr, &snapshot));
                tx.open_db(None).unwrap();
                drop(tx);

                assert!(!read_transactions.is_active(tx_ptr, &snapshot));

                // A collected entry must not match a later transaction reusing its pooled handle.
                let tx = env.begin_ro_txn().unwrap();
                assert_eq!(tx.txn() as usize, tx_ptr);
                assert!(!read_transactions.is_active(tx_ptr, &snapshot));
                tx.open_db(None).unwrap();
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
                // Create a read-only transaction and observe it's in the list of active
                // transactions.
                let tx = env.begin_ro_txn().unwrap();
                let tx_ptr = tx.txn() as usize;
                assert!(read_transactions.active.contains_key(&tx_ptr));

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

        /// Commits and drops read transactions around their timeout while the monitor is timing
        /// them out. The monitor must not hold a lock on the active transactions while waiting
        /// for a transaction lock, otherwise it deadlocks with commit and drop.
        #[test]
        fn txn_manager_read_transactions_timeout_commit_drop_race() {
            const MAX_DURATION: Duration = Duration::from_millis(10);
            const THREADS: usize = 16;
            const RUN_FOR: Duration = Duration::from_secs(2);

            let dir = tempdir().unwrap();
            let env = Environment::builder()
                .set_max_read_transaction_duration(MaxReadTransactionDuration::Set(MAX_DURATION))
                .open(dir.path())
                .unwrap();

            let (done_tx, done_rx) = mpsc::channel();
            for thread_id in 0..THREADS {
                let env = env.clone();
                let done_tx = done_tx.clone();
                thread::spawn(move || {
                    let deadline = Instant::now() + RUN_FOR;
                    let mut i = thread_id;
                    while Instant::now() < deadline {
                        let tx = env.begin_ro_txn().unwrap();
                        // Finish the transaction right around the moment it expires.
                        sleep(
                            MAX_DURATION - Duration::from_millis(3) +
                                Duration::from_millis(i as u64 % 6),
                        );
                        // The transaction might have been timed out already.
                        if i % 2 == 0 {
                            if let Err(err) = tx.commit() {
                                assert_eq!(err, Error::ReadTransactionTimeout);
                            }
                        } else {
                            drop(tx);
                        }
                        i += 1;
                    }
                    done_tx.send(()).unwrap();
                });
            }
            drop(done_tx);

            for _ in 0..THREADS {
                done_rx
                    .recv_timeout(Duration::from_secs(30))
                    .expect("read transaction timeout monitor deadlocked");
            }

            // Every transaction was finished, so nothing must be left behind.
            let read_transactions = env.txn_manager().read_transactions.as_ref().unwrap();
            assert!(read_transactions.active.is_empty());
            assert!(read_transactions.timed_out_not_aborted.is_empty());
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

        #[test]
        fn read_transaction_monitor_stops_on_last_environment_drop() {
            let dir = tempdir().unwrap();
            let env = Environment::builder()
                .set_max_read_transaction_duration(MaxReadTransactionDuration::Set(
                    Duration::from_secs(60),
                ))
                .open(dir.path())
                .unwrap();
            let transactions =
                Arc::downgrade(&env.txn_manager().read_transactions.as_ref().unwrap().transactions);
            let clone = env.clone();
            drop(env);
            assert!(transactions.upgrade().is_some());

            drop(clone);
            assert!(transactions.upgrade().is_none(), "monitor retained transaction state");
        }

        #[test]
        fn read_transaction_monitor_shutdown_wakes_waiter() {
            let transactions = Arc::new(ReadTransactions::new(Duration::from_secs(60)));
            let monitor = transactions.start_monitor();
            // Retaining the state proves shutdown does not depend on its reference count.
            let transactions = monitor.transactions.clone();
            let started = Instant::now();
            drop(monitor);
            assert!(started.elapsed() < READ_TRANSACTIONS_CHECK_INTERVAL);
            assert_eq!(Arc::strong_count(&transactions), 1);
        }
    }
}
