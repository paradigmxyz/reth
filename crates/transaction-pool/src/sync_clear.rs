//! Synchronous transaction pool clearing support.
//!
//! This module provides channel types for synchronously clearing the transaction pool
//! after successful forkchoice updates. The engine tree handler (running in a blocking
//! thread) can send clear commands and wait for acknowledgment from a dedicated async
//! pool clearing task.

use std::{sync::mpsc, time::Duration};
use tokio::sync::mpsc as tokio_mpsc;

/// Default timeout for pool clear operations.
/// This prevents indefinite blocking if the clearing task is stuck or crashed.
const POOL_CLEAR_TIMEOUT: Duration = Duration::from_secs(30);

/// Command to synchronously clear the transaction pool.
///
/// Contains an acknowledgment channel that must be used to signal completion.
pub struct PoolClearCommand {
    ack: mpsc::Sender<()>,
}

impl std::fmt::Debug for PoolClearCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolClearCommand").finish_non_exhaustive()
    }
}

impl PoolClearCommand {
    /// Acknowledge that the pool has been cleared.
    ///
    /// Sends the acknowledgment signal to the waiting engine thread.
    /// Errors are ignored as they only occur if the engine thread
    /// has already dropped the receiver (e.g., during shutdown).
    pub fn acknowledge(self) {
        let _ = self.ack.send(());
    }
}

/// Handle for the engine to send synchronous pool clear commands.
///
/// This handle can be cloned and used from the blocking engine tree thread.
/// The `clear_and_wait` method blocks until the pool acknowledges clearing.
#[derive(Clone)]
pub struct PoolClearHandle {
    sender: tokio_mpsc::UnboundedSender<PoolClearCommand>,
}

impl std::fmt::Debug for PoolClearHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolClearHandle").finish_non_exhaustive()
    }
}

impl PoolClearHandle {
    /// Sends a clear command and blocks until the pool acknowledges or timeout.
    ///
    /// This method is designed to be called from blocking contexts (like the
    /// engine tree handler thread). It will block the current thread until
    /// the pool task clears all transactions and sends an acknowledgment,
    /// or until the timeout expires.
    ///
    /// Returns `true` if the pool acknowledged clearing, `false` if the
    /// operation timed out or the receiver was dropped.
    pub fn clear_and_wait(&self) -> bool {
        let (ack_tx, ack_rx) = mpsc::channel();
        if self.sender.send(PoolClearCommand { ack: ack_tx }).is_ok() {
            // Wait for acknowledgment with timeout to prevent indefinite blocking
            ack_rx.recv_timeout(POOL_CLEAR_TIMEOUT).is_ok()
        } else {
            // Receiver dropped, clearing task not running
            false
        }
    }
}

/// Receiver for the pool clearing task to receive clear commands.
///
/// This receiver is used in async contexts to receive clear commands from the engine.
#[derive(Debug)]
pub struct PoolClearReceiver {
    receiver: tokio_mpsc::UnboundedReceiver<PoolClearCommand>,
}

impl PoolClearReceiver {
    /// Receives the next clear command asynchronously.
    ///
    /// Returns `None` if all senders have been dropped.
    pub async fn recv(&mut self) -> Option<PoolClearCommand> {
        self.receiver.recv().await
    }

    /// Attempts to receive a clear command without blocking.
    ///
    /// Returns `Ok(cmd)` if a command is available, `Err(TryRecvError)` otherwise.
    pub fn try_recv(&mut self) -> Result<PoolClearCommand, tokio_mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }
}

/// Creates a linked handle-receiver pair for synchronous pool clearing.
///
/// The handle should be passed to the engine service, while the receiver
/// should be used to spawn a dedicated pool clearing task.
pub fn pool_clear_channel() -> (PoolClearHandle, PoolClearReceiver) {
    let (tx, rx) = tokio_mpsc::unbounded_channel();
    (PoolClearHandle { sender: tx }, PoolClearReceiver { receiver: rx })
}
