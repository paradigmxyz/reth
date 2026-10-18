use alloy_eips::BlockNumHash;
use tokio::sync::mpsc;

/// Events emitted by an `ExEx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExExEvent {
    /// Highest block processed by the `ExEx`.
    ///
    /// The `ExEx` must guarantee that it will not require all earlier blocks in the future,
    /// meaning that Reth is allowed to prune them.
    ///
    /// On reorgs, it's possible for the height to go down.
    FinishedHeight(BlockNumHash),
}

/// Submits an execution extension's events to its manager.
///
/// Obtained from [`crate::ExExHandle::new`]. Clones submit to the same manager and can be retained
/// independently of the extension context. Sending an event acknowledges only that it was queued,
/// not that the manager processed it. The queue has no capacity limit.
///
/// An existing Tokio event channel can be adapted with [`From`] when constructing a custom context
/// or test harness.
#[derive(Debug, Clone)]
pub struct ExExEventSender {
    sender: mpsc::UnboundedSender<ExExEvent>,
}

impl ExExEventSender {
    /// Queues an event for the manager without waiting for it to be processed.
    ///
    /// Returns the unsent event if the manager is no longer accepting events.
    pub fn send(&self, event: ExExEvent) -> Result<(), ExExEventSendError> {
        self.sender.send(event).map_err(|error| ExExEventSendError { event: error.0 })
    }

    /// Reports the highest processed block whose earlier blocks are safe to prune.
    ///
    /// The height may decrease after a reorg. Success means the report was queued, not that the
    /// manager processed it. Returns the unsent [`ExExEvent::FinishedHeight`] if the manager is no
    /// longer accepting events.
    pub fn send_finished_height(&self, height: BlockNumHash) -> Result<(), ExExEventSendError> {
        self.send(ExExEvent::FinishedHeight(height))
    }
}

impl From<mpsc::UnboundedSender<ExExEvent>> for ExExEventSender {
    fn from(sender: mpsc::UnboundedSender<ExExEvent>) -> Self {
        Self { sender }
    }
}

/// An event that could not be queued because its execution extension manager is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("execution extension manager is no longer accepting events")]
pub struct ExExEventSendError {
    /// The event that was not accepted by the manager.
    pub event: ExExEvent,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn closed_manager_returns_unsent_event() {
        let (sender, receiver) = mpsc::unbounded_channel();
        let events = ExExEventSender::from(sender);
        let cloned_events = events.clone();
        drop(receiver);
        let height = BlockNumHash::new(42, B256::repeat_byte(1));
        let event = ExExEvent::FinishedHeight(height);

        assert_eq!(cloned_events.send(event), Err(ExExEventSendError { event }));
        assert_eq!(events.send_finished_height(height), Err(ExExEventSendError { event }));
    }
}
