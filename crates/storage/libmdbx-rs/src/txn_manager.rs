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

    fn start_message_listener(&self, env: EnvPtr, rx: Receiver<TxnManagerMessage>) {
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
                        }
                        TxnManagerMessage::Abort { tx, sender } => {
                            let result = mdbx_result(unsafe { ffi::mdbx_txn_abort(tx.0) });
                            sender.send(result).unwrap();
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
    use crate::{error::mdbx_result, txn_manager::TxnManager, Error};
    use dashmap::{DashMap, DashSet};
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    use tracing::{error, trace, warn};

    impl TxnManager {
        pub(crate) fn with_max_read_transaction_duration(
            mut self,
            duration: Duration,
        ) -> TxnManager {
            let read_transactions = Arc::new(ReadTransactions::new(duration));
            read_transactions.clone().start_monitor();
            self.read_transactions = Some(read_transactions);

            self
        }

        pub(crate) fn add_active_read_transaction(&self, ptr: *mut ffi::MDBX_txn) {
            if let Some(read_transactions) = &self.read_transactions {
                read_transactions.add_active(ptr);
            }
        }

        pub(crate) fn remove_active_read_transaction(
            &self,
            ptr: *mut ffi::MDBX_txn,
        ) -> Option<(usize, Instant)> {
            self.read_transactions.as_ref()?.remove_active(ptr)
        }

        pub(crate) fn remove_aborted_read_transaction(
            &self,
            ptr: *mut ffi::MDBX_txn,
        ) -> Option<usize> {
            self.read_transactions.as_ref()?.remove_aborted(ptr)
        }
    }

    #[derive(Debug, Default)]
    pub(crate) struct ReadTransactions {
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

        pub(crate) fn add_active(&self, ptr: *mut ffi::MDBX_txn) {
            let _ = self.active.insert(ptr as usize, Instant::now());
        }

        pub(crate) fn remove_active(&self, ptr: *mut ffi::MDBX_txn) -> Option<(usize, Instant)> {
            self.active.remove(&(ptr as usize))
        }

        pub(crate) fn add_aborted(&self, ptr: *mut ffi::MDBX_txn) {
            self.aborted.insert(ptr as usize);
        }

        pub(crate) fn remove_aborted(&self, ptr: *mut ffi::MDBX_txn) -> Option<usize> {
            self.aborted.remove(&(ptr as usize))
        }

        pub(super) fn start_monitor(self: Arc<Self>) {
            std::thread::spawn(move || {
                let mut aborted_active = Vec::new();

                loop {
                    let now = Instant::now();

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

                    std::thread::sleep(Duration::from_millis(500));
                }
            });
        }
    }
}
