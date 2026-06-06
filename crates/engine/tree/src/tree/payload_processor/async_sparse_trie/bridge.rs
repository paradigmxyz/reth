use super::BRIDGE_RECV_TIMEOUT;
use crate::tree::payload_processor::multiproof::StateRootMessage;
use crossbeam_channel::{Receiver as CrossbeamReceiver, RecvTimeoutError};
use reth_trie_parallel::proof_task::ProofResultMessage;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle as StdJoinHandle},
};
use tokio::sync::mpsc::UnboundedSender;

pub(super) struct BridgeHandle {
    stop: Arc<AtomicBool>,
    handle: Option<StdJoinHandle<()>>,
}

impl BridgeHandle {
    pub(super) fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub(super) fn spawn_state_message_bridge(
    rx: CrossbeamReceiver<StateRootMessage>,
    tx: UnboundedSender<StateRootMessage>,
) -> BridgeHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let handle = thread::Builder::new()
        .name("async-sr-state-bridge".to_string())
        .spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match rx.recv_timeout(BRIDGE_RECV_TIMEOUT) {
                    Ok(message) => {
                        let finished = matches!(message, StateRootMessage::FinishedStateUpdates);
                        if tx.send(message).is_err() || finished {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .ok();

    BridgeHandle { stop, handle }
}

pub(super) fn spawn_proof_result_bridge(
    rx: CrossbeamReceiver<ProofResultMessage>,
    tx: UnboundedSender<ProofResultMessage>,
) -> BridgeHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let handle = thread::Builder::new()
        .name("async-sr-proof-bridge".to_string())
        .spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match rx.recv_timeout(BRIDGE_RECV_TIMEOUT) {
                    Ok(message) => {
                        if tx.send(message).is_err() {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .ok();

    BridgeHandle { stop, handle }
}
