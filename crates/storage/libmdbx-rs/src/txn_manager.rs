use crate::{
    environment::EnvPtr,
    error::{mdbx_result, Result},
    CommitLatency,
};
use std::{
    ptr,
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

/// Manages transactions by doing two things:
/// - Opening, aborting, and committing transactions using [TxnManager::send_message] with the
///   corresponding [TxnManagerMessage]
/// - Aborting long-lived read transactions (if the `read-tx-timeouts` feature is enabled and
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

    /// Spawns a new thread with [std::thread::spawn] that listens to incoming [TxnManagerMessage]
    /// messages, executes an FFI function, and returns the result on the provided channel.
    ///
    /// - [TxnManagerMessage::Begin] opens a new transaction with [ffi::mdbx_txn_begin_ex]
    /// - [TxnManagerMessage::Abort] aborts a transaction with [ffi::mdbx_txn_abort]
    /// - [TxnManagerMessage::Commit] commits a transaction with [ffi::mdbx_txn_commit_ex]
    fn start_message_listener(&self, env: EnvPtr, rx: Receiver<TxnManagerMessage>) {
        let read_transactions = self.read_transactions.clone();
        std::thread::spawn(move || {
            #[allow(clippy::redundant_locals)]
            let env = env;
            loop {
                match rx.recv() {
                    Ok(msg) => match msg {
                        TxnManagerMessage::Begin { parent, flags, sender } => {
                            let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
                            sender
                                .send(
                                    mdbx_result(unsafe {
                                        ffi::mdbx_txn_begin_ex(
                                            env.0,
                                            parent.0,
                                            flags,
                                            &mut txn,
                                            ptr::null_mut(),
                                        )
                                    })
                                    .map(|_| TxnPtr(txn)),
                                )
                                .unwrap();

                            #[cfg(feature = "read-tx-timeouts")]
                            {
                                use crate::transaction::TransactionKind;

                                if flags == crate::transaction::RO::OPEN_FLAGS {
                                    if let Some(read_transactions) = &read_transactions {
                                        read_transactions.add_active(txn);
                                    }
                                }
                            }
                        }
                        TxnManagerMessage::Abort { tx, sender } => {
                            #[cfg(feature = "read-tx-timeouts")]
                            if let Some(read_transactions) = &read_transactions {
                                read_transactions.remove_active(tx.0);
                            }

                            sender.send(mdbx_result(unsafe { ffi::mdbx_txn_abort(tx.0) })).unwrap();
                        }
                        TxnManagerMessage::Commit { tx, sender } => {
                            #[cfg(feature = "read-tx-timeouts")]
                            if let Some(read_transactions) = &read_transactions {
                                read_transactions.remove_active(tx.0);
                            }

                            sender
                                .send({
                                    let mut latency = CommitLatency::new();
                                    mdbx_result(unsafe {
                                        ffi::mdbx_txn_commit_ex(tx.0, latency.mdb_commit_latency())
                                    })
                                    .map(|v| (v, latency))
                                })
                                .unwrap();
                        }
                    },
                    Err(_) => return,
                }
            }
        });
    }

    pub(crate) fn send_message(&self, message: TxnManagerMessage) {
        self.sender.send(message).unwrap()
    }
}

#[cfg(feature = "read-tx-timeouts")]
mod read_transactions {
    use crate::{error::mdbx_result, txn_manager::TxnManager, Error};
    use dashmap::{DashMap, DashSet};
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    use tracing::{error, trace, warn};

    const READ_TRANSACTIONS_CHECK_INTERVAL: Duration = Duration::from_secs(5);

    impl TxnManager {
        /// Sets the maximum duration that a read transaction can be open.
        pub(crate) fn with_max_read_transaction_duration(
            mut self,
            duration: Duration,
        ) -> TxnManager {
            let read_transactions = Arc::new(ReadTransactions::new(duration));
            read_transactions.clone().start_monitor();
            self.read_transactions = Some(read_transactions);

            self
        }

        /// Adds a new transaction to the list of active read transactions.
        pub(crate) fn add_active_read_transaction(&self, ptr: *mut ffi::MDBX_txn) {
            if let Some(read_transactions) = &self.read_transactions {
                read_transactions.add_active(ptr);
            }
        }

        /// Removes a transaction from the list of active read transactions.
        pub(crate) fn remove_active_read_transaction(
            &self,
            ptr: *mut ffi::MDBX_txn,
        ) -> Option<(usize, Instant)> {
            self.read_transactions.as_ref()?.remove_active(ptr)
        }

        /// Removes a transaction from the list of aborted read transactions.
        pub(crate) fn remove_aborted_read_transaction(
            &self,
            ptr: *mut ffi::MDBX_txn,
        ) -> Option<usize> {
            self.read_transactions.as_ref()?.remove_aborted(ptr)
        }
    }

    #[derive(Debug, Default)]
    pub(super) struct ReadTransactions {
        /// Maximum duration that a read transaction can be open until the
        /// [ReadTransactions::start_monitor] aborts it.
        max_duration: Duration,
        /// List of currently active read transactions.
        ///
        /// We store `usize` instead of a raw pointer as a key, because pointers are not
        /// comparable. The time of transaction opening is stored as a value.
        active: DashMap<usize, Instant>,
        /// List of read transactions aborted by the [ReadTransactions::start_monitor].
        /// We keep them until user tries to abort the transaction, so we're able to report a nice
        /// [Error::ReadTransactionAborted] error.
        ///
        /// We store `usize` instead of a raw pointer, because pointers are not comparable.
        aborted: DashSet<usize>,
    }

    impl ReadTransactions {
        pub(super) fn new(max_duration: Duration) -> Self {
            Self { max_duration, ..Default::default() }
        }

        /// Adds a new transaction to the list of active read transactions.
        pub(super) fn add_active(&self, ptr: *mut ffi::MDBX_txn) {
            let _ = self.active.insert(ptr as usize, Instant::now());
        }

        /// Removes a transaction from the list of active read transactions.
        pub(super) fn remove_active(&self, ptr: *mut ffi::MDBX_txn) -> Option<(usize, Instant)> {
            self.active.remove(&(ptr as usize))
        }

        /// Adds a new transaction to the list of aborted read transactions.
        pub(super) fn add_aborted(&self, ptr: *mut ffi::MDBX_txn) {
            self.aborted.insert(ptr as usize);
        }

        /// Removes a transaction from the list of aborted read transactions.
        pub(super) fn remove_aborted(&self, ptr: *mut ffi::MDBX_txn) -> Option<usize> {
            self.aborted.remove(&(ptr as usize))
        }

        /// Spawns a new thread with [std::thread::spawn] that monitors the list of active read
        /// transactions and aborts those that are open for longer than
        /// `ReadTransactions.max_duration`.
        ///
        /// Aborted transaction pointers are placed into the list of aborted read transactions, and
        /// removed from this list by [crate::error::mdbx_result_with_tx_kind] when the user tries
        /// to use it.
        pub(super) fn start_monitor(self: Arc<Self>) {
            std::thread::spawn(move || {
                let mut aborted_active = Vec::new();

                loop {
                    let now = Instant::now();
                    let mut max_active_transaction_duration = None;

                    // Iterate through active read transactions and abort those that's open for
                    // longer than `self.max_duration`.
                    for entry in self.active.iter() {
                        let (ptr, start) = entry.pair();
                        let duration = now - *start;

                        if duration > self.max_duration {
                            let ptr = *ptr as *mut ffi::MDBX_txn;

                            // Add the transaction to the list of aborted transactions, so further
                            // usages report the correct error when the transaction is closed.
                            self.add_aborted(ptr);

                            // Abort the transaction
                            let result = mdbx_result(unsafe { ffi::mdbx_txn_abort(ptr) });

                            // Add the transaction to `aborted_active`. We can't remove it instantly
                            // from the list of active transactions, because we iterate through it.
                            aborted_active.push((ptr, duration, result.err()));
                        } else {
                            max_active_transaction_duration = Some(
                                duration.max(max_active_transaction_duration.unwrap_or_default()),
                            );
                        }
                    }

                    // Walk through aborted transactions, and delete them from the list of active
                    // transactions.
                    for (ptr, open_duration, err) in aborted_active.iter().copied() {
                        // Try deleting the transaction from the list of active transactions.
                        let was_in_active = self.remove_active(ptr).is_some();
                        if let Some(err) = err {
                            // If there was an error when aborting the transaction, we need to
                            // remove it from the list of aborted transactions, because otherwise it
                            // will stay there forever.
                            self.remove_aborted(ptr);
                            if was_in_active && err != Error::BadSignature {
                                // If the transaction was in the list of active transactions and the
                                // error code is not `EBADSIGN`, then user didn't abort it.
                                error!(target: "libmdbx", ?err, ?open_duration, "Failed to abort the long-lived read transactions");
                            }
                        } else {
                            // Happy path, the transaction has been aborted by us with no errors.
                            warn!(target: "libmdbx", ?open_duration, "Long-lived read transactions has been aborted");
                        }
                    }

                    // Clear the list of aborted transactions, but not de-allocate the reserved
                    // capacity to save on further pushes.
                    aborted_active.clear();

                    if !self.active.is_empty() || !self.aborted.is_empty() {
                        trace!(
                            target: "libmdbx",
                            elapsed = ?now.elapsed(),
                            active = ?self.active.iter().map(|entry| {
                                let (ptr, start) = entry.pair();
                                (*ptr, start.elapsed())
                            }).collect::<Vec<_>>(),
                            aborted = ?self.aborted.iter().map(|entry| *entry).collect::<Vec<_>>(),
                            "Read transactions"
                        );
                    }

                    // Sleep not more than `READ_TRANSACTIONS_CHECK_INTERVAL`, but at least until
                    // the closest deadline of an active read transaction
                    let duration_until_closest_deadline =
                        self.max_duration - max_active_transaction_duration.unwrap_or_default();
                    std::thread::sleep(
                        READ_TRANSACTIONS_CHECK_INTERVAL.min(duration_until_closest_deadline),
                    );
                }
            });
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::{
            txn_manager::read_transactions::READ_TRANSACTIONS_CHECK_INTERVAL, Environment, Error,
            MaxReadTransactionDuration,
        };
        use std::{thread::sleep, time::Duration};
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

                tx.open_db(None).unwrap();
                drop(tx);

                assert!(!read_transactions.active.contains_key(&tx_ptr));
                assert!(!read_transactions.aborted.contains(&tx_ptr));
            }

            // Create a read-only transaction, successfully use it, close it by committing.
            {
                let tx = env.begin_ro_txn().unwrap();
                let tx_ptr = tx.txn() as usize;
                assert!(read_transactions.active.contains_key(&tx_ptr));

                tx.open_db(None).unwrap();
                tx.commit().unwrap();

                assert!(!read_transactions.active.contains_key(&tx_ptr));
                assert!(!read_transactions.aborted.contains(&tx_ptr));
            }

            // Create a read-only transaction, wait until `MAX_DURATION` time is elapsed so the
            // manager kills it, use it and observe the `Error::ReadTransactionAborted` error.
            {
                let tx = env.begin_ro_txn().unwrap();
                let tx_ptr = tx.txn() as usize;
                assert!(read_transactions.active.contains_key(&tx_ptr));

                sleep(MAX_DURATION + READ_TRANSACTIONS_CHECK_INTERVAL);

                assert!(!read_transactions.active.contains_key(&tx_ptr));
                assert!(read_transactions.aborted.contains(&tx_ptr));

                assert_eq!(tx.open_db(None).err(), Some(Error::ReadTransactionAborted));
                assert!(!read_transactions.active.contains_key(&tx_ptr));
                assert!(!read_transactions.aborted.contains(&tx_ptr));
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
            sleep(READ_TRANSACTIONS_CHECK_INTERVAL);
            assert!(tx.commit().is_ok())
        }
    }
}
