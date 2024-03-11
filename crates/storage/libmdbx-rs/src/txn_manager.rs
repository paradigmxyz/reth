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
        std::thread::spawn(move || {
            #[allow(clippy::redundant_locals)]
            let env = env;
            loop {
                match rx.recv() {
                    Ok(msg) => match msg {
                        TxnManagerMessage::Begin { parent, flags, sender } => {
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
                            sender.send(mdbx_result(unsafe { ffi::mdbx_txn_abort(tx.0) })).unwrap();
                        }
                        TxnManagerMessage::Commit { tx, sender } => {
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
    use crate::{
        environment::EnvPtr, error::mdbx_result, transaction::TransactionPtr,
        txn_manager::TxnManager,
    };
    use dashmap::{DashMap, DashSet};
    use std::{
        sync::{mpsc::sync_channel, Arc},
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
            read_transactions.clone().start_monitor();

            let (tx, rx) = sync_channel(0);

            let txn_manager = Self { sender: tx, read_transactions: Some(read_transactions) };

            txn_manager.start_message_listener(env, rx);

            txn_manager
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
        pub(crate) fn remove_active_read_transaction(
            &self,
            ptr: *mut ffi::MDBX_txn,
        ) -> Option<(usize, (TransactionPtr, Instant))> {
            self.read_transactions.as_ref()?.remove_active(ptr)
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
        /// [ReadTransactions::start_monitor] aborts it.
        max_duration: Duration,
        /// List of currently active read transactions.
        ///
        /// We store `usize` instead of a raw pointer as a key, because pointers are not
        /// comparable. The time of transaction opening is stored as a value.
        active: DashMap<usize, (TransactionPtr, Instant)>,
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
            let _ = self.active.insert(ptr as usize, (tx, Instant::now()));
        }

        /// Removes a transaction from the list of active read transactions.
        pub(super) fn remove_active(
            &self,
            ptr: *mut ffi::MDBX_txn,
        ) -> Option<(usize, (TransactionPtr, Instant))> {
            self.timed_out_not_aborted.remove(&(ptr as usize));
            self.active.remove(&(ptr as usize))
        }

        /// Returns the number of timed out transactions that were not aborted by the user yet.
        pub(super) fn timed_out_not_aborted(&self) -> usize {
            self.timed_out_not_aborted.len()
        }

        /// Spawns a new thread with [std::thread::spawn] that monitors the list of active read
        /// transactions and timeouts those that are open for longer than
        /// `ReadTransactions.max_duration`.
        pub(super) fn start_monitor(self: Arc<Self>) {
            std::thread::spawn(move || {
                let mut timed_out_active = Vec::new();

                loop {
                    let now = Instant::now();
                    let mut max_active_transaction_duration = None;

                    // Iterate through active read transactions and time out those that's open for
                    // longer than `self.max_duration`.
                    for entry in self.active.iter() {
                        let (tx, start) = entry.value();
                        let duration = now - *start;

                        if duration > self.max_duration {
                            let result = tx.txn_execute_fail_on_timeout(|txn_ptr| {
                                // Time out the transaction.
                                //
                                // We use `mdbx_txn_reset` instead of `mdbx_txn_abort` here to
                                // prevent MDBX from reusing the pointer of the aborted
                                // transaction for new read-only transactions. This is
                                // important because we store the pointer in the `active` list
                                // and assume that it is unique.
                                //
                                // See https://erthink.github.io/libmdbx/group__c__transactions.html#gae9f34737fe60b0ba538d5a09b6a25c8d for more info.
                                let result = mdbx_result(unsafe { ffi::mdbx_txn_reset(txn_ptr) });
                                if result.is_ok() {
                                    tx.set_timed_out();
                                }
                                (txn_ptr, duration, result)
                            });

                            match result {
                                Ok((txn_ptr, duration, error)) => {
                                    // Add the transaction to `timed_out_active`. We can't remove it
                                    // instantly from the list of active transactions, because we
                                    // iterate through it.
                                    timed_out_active.push((txn_ptr, duration, error));
                                }
                                Err(err) => {
                                    error!(target: "libmdbx", %err, "Failed to abort the long-lived read transaction")
                                }
                            }
                        } else {
                            max_active_transaction_duration = Some(
                                duration.max(max_active_transaction_duration.unwrap_or_default()),
                            );
                        }
                    }

                    // Walk through timed out transactions, and delete them from the list of active
                    // transactions.
                    for (ptr, open_duration, err) in timed_out_active.iter().copied() {
                        // Try deleting the transaction from the list of active transactions.
                        let was_in_active = self.remove_active(ptr).is_some();
                        if let Err(err) = err {
                            if was_in_active {
                                // If the transaction was in the list of active transactions,
                                // then user didn't abort it and we failed to do so.
                                error!(target: "libmdbx", %err, ?open_duration, "Failed to time out the long-lived read transaction");
                            }
                        } else {
                            // Happy path, the transaction has been timed out by us with no errors.
                            warn!(target: "libmdbx", ?open_duration, "Long-lived read transaction has been timed out");
                            // Add transaction to the list of timed out transactions that were not
                            // aborted by the user yet.
                            self.timed_out_not_aborted.insert(ptr as usize);
                        }
                    }

                    // Clear the list of timed out transactions, but not de-allocate the reserved
                    // capacity to save on further pushes.
                    timed_out_active.clear();

                    if !self.active.is_empty() {
                        trace!(
                            target: "libmdbx",
                            elapsed = ?now.elapsed(),
                            active = ?self.active.iter().map(|entry| {
                                let (tx, start) = entry.value();
                                (tx.clone(), start.elapsed())
                            }).collect::<Vec<_>>(),
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
                sleep(MAX_DURATION + READ_TRANSACTIONS_CHECK_INTERVAL);

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
