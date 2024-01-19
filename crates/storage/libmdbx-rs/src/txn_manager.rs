use crate::{
    environment::EnvPtr,
    error::{mdbx_result, Result},
    CommitLatency,
};
use std::{
    ptr,
    sync::{
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc,
    },
    time::Duration,
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
    read_transactions: Arc<read_transactions::ReadTransactions>,
}

impl TxnManager {
    pub(crate) fn new(
        env: EnvPtr,
        #[cfg(feature = "read-tx-timeouts")] max_read_transaction_duration: Duration,
    ) -> Self {
        let (tx, rx) = sync_channel(0);
        let txn_manager = Self {
            sender: tx,
            #[cfg(feature = "read-tx-timeouts")]
            read_transactions: Arc::new(read_transactions::ReadTransactions::new(
                max_read_transaction_duration,
            )),
        };

        txn_manager.start_message_listener(env, rx);
        #[cfg(feature = "read-tx-timeouts")]
        txn_manager.read_transactions.clone().start_monitor(txn_manager.sender.clone());

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
                            #[allow(clippy::redundant_locals)]
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
    use crate::txn_manager::{TxnManager, TxnManagerMessage, TxnPtr};
    use dashmap::{DashMap, DashSet};
    use std::{
        sync::{
            mpsc::{sync_channel, SyncSender},
            Arc,
        },
        time::{Duration, Instant},
    };

    impl TxnManager {
        pub(crate) fn add_active_read_transaction(&self, ptr: *mut ffi::MDBX_txn) {
            self.read_transactions.add_active(ptr);
        }

        pub(crate) fn remove_active_read_transaction(
            &self,
            ptr: *mut ffi::MDBX_txn,
        ) -> Option<(usize, Instant)> {
            self.read_transactions.remove_active(ptr)
        }

        pub(crate) fn remove_aborted_read_transaction(
            &self,
            ptr: *mut ffi::MDBX_txn,
        ) -> Option<usize> {
            self.read_transactions.remove_aborted(ptr)
        }
    }

    #[derive(Debug, Default)]
    pub(crate) struct ReadTransactions {
        max_duration: Duration,
        active: DashMap<usize, Instant>,
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

        pub(super) fn start_monitor(
            self: Arc<Self>,
            txn_manager_messages: SyncSender<TxnManagerMessage>,
        ) {
            std::thread::spawn(move || loop {
                let now = Instant::now();

                for entry in self.active.iter() {
                    let (ptr, start) = entry.pair();
                    let ptr = *ptr as *mut ffi::MDBX_txn;
                    if (now - *start) > self.max_duration {
                        let (sender, rx) = sync_channel(0);
                        txn_manager_messages
                            .send(TxnManagerMessage::Abort { tx: TxnPtr(ptr), sender })
                            .unwrap();
                        rx.recv().unwrap().unwrap();

                        self.add_aborted(ptr);
                        self.remove_active(ptr);
                    }
                }

                std::thread::sleep(Duration::from_millis(10));
            });
        }
    }
}
