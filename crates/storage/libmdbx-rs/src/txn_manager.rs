use crate::{
    environment::EnvPtr,
    error::{mdbx_result, Result},
    CommitLatency,
};
use dashmap::DashMap;
use std::{
    backtrace::Backtrace,
    ptr,
    sync::{
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc,
    },
    time::{Duration, Instant},
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

#[derive(Debug, Clone)]
pub(crate) struct TxnManager {
    sender: SyncSender<TxnManagerMessage>,
    read_transactions: Arc<DashMap<usize, (Instant, Backtrace)>>,
    max_read_transaction_duration: Duration,
}

impl TxnManager {
    pub(crate) fn new(env: EnvPtr, max_read_transaction_duration: Duration) -> Self {
        let (tx, rx) = sync_channel(0);
        let txn_manager = Self {
            sender: tx,
            read_transactions: Arc::new(DashMap::new()),
            max_read_transaction_duration,
        };

        txn_manager.start_message_listener(env, rx);
        txn_manager.clone().start_read_transactions_monitor();

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

    fn start_read_transactions_monitor(self) {
        std::thread::spawn(move || loop {
            let now = Instant::now();

            for entry in self.read_transactions.iter() {
                let (ptr, (start, backtrace)) = entry.pair();
                if (now - *start) > self.max_read_transaction_duration {
                    let (sender, rx) = sync_channel(0);
                    self.send_message(TxnManagerMessage::Abort {
                        tx: TxnPtr(*ptr as *mut ffi::MDBX_txn),
                        sender,
                    });
                    rx.recv().unwrap().unwrap();
                    println!("{backtrace}");

                    self.read_transactions.remove(ptr);
                }
            }

            std::thread::sleep(Duration::from_millis(10));
        });
    }

    pub(crate) fn send_message(&self, message: TxnManagerMessage) {
        self.sender.send(message).unwrap()
    }

    pub(crate) fn add_read_transaction(&self, ptr: *mut ffi::MDBX_txn) {
        let _ = self
            .read_transactions
            .insert(ptr as usize, (Instant::now(), Backtrace::force_capture()));
    }

    pub(crate) fn remove_read_transaction(&self, ptr: *mut ffi::MDBX_txn) {
        let _ = self.read_transactions.remove(&(ptr as usize));
    }
}
