use crate::{ExExEvent, ExExNotification, FinishedExExHeight};
use metrics::Gauge;
use reth_metrics::{metrics::Counter, Metrics};
use reth_primitives::BlockNumber;
use reth_tracing::tracing::debug;
use std::{
    collections::VecDeque,
    future::{poll_fn, Future},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
};
use tokio::sync::{
    mpsc::{self, error::SendError, Receiver, UnboundedReceiver, UnboundedSender},
    watch,
};
use tokio_util::sync::{PollSendError, PollSender, ReusableBoxFuture};

/// Metrics for an `ExEx`.
#[derive(Metrics)]
#[metrics(scope = "exex")]
struct ExExMetrics {
    /// The total number of notifications sent to an `ExEx`.
    notifications_sent_total: Counter,
    /// The total number of events an `ExEx` has sent to the manager.
    events_sent_total: Counter,
}

/// A handle to an `ExEx` used by the [`ExExManager`] to communicate with `ExEx`'s.
///
/// A handle should be created for each `ExEx` with a unique ID. The channels returned by
/// [`ExExHandle::new`] should be given to the `ExEx`, while the handle itself should be given to
/// the manager in [`ExExManager::new`].
#[derive(Debug)]
pub struct ExExHandle {
    /// The execution extension's ID.
    id: String,
    /// Metrics for an `ExEx`.
    metrics: ExExMetrics,

    /// Channel to send [`ExExNotification`]s to the `ExEx`.
    sender: PollSender<ExExNotification>,
    /// Channel to receive [`ExExEvent`]s from the `ExEx`.
    receiver: UnboundedReceiver<ExExEvent>,
    /// The ID of the next notification to send to this `ExEx`.
    next_notification_id: usize,

    /// The finished block number of the `ExEx`.
    ///
    /// If this is `None`, the `ExEx` has not emitted a `FinishedHeight` event.
    finished_height: Option<BlockNumber>,
}

impl ExExHandle {
    /// Create a new handle for the given `ExEx`.
    ///
    /// Returns the handle, as well as a [`UnboundedSender`] for [`ExExEvent`]s and a
    /// [`Receiver`] for [`ExExNotification`]s that should be given to the `ExEx`.
    pub fn new(id: String) -> (Self, UnboundedSender<ExExEvent>, Receiver<ExExNotification>) {
        let (notification_tx, notification_rx) = mpsc::channel(1);
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        (
            Self {
                id: id.clone(),
                metrics: ExExMetrics::new_with_labels(&[("exex", id)]),
                sender: PollSender::new(notification_tx),
                receiver: event_rx,
                next_notification_id: 0,
                finished_height: None,
            },
            event_tx,
            notification_rx,
        )
    }

    /// Reserves a slot in the `PollSender` channel and sends the notification if the slot was
    /// successfully reserved.
    ///
    /// When the notification is sent, it is considered delivered.
    fn send(
        &mut self,
        cx: &mut Context<'_>,
        (notification_id, notification): &(usize, ExExNotification),
    ) -> Poll<Result<(), PollSendError<ExExNotification>>> {
        if let Some(finished_height) = self.finished_height {
            match notification {
                ExExNotification::ChainCommitted { new } => {
                    // Skip the chain commit notification if the finished height of the ExEx is
                    // higher than or equal to the tip of the new notification.
                    // I.e., the ExEx has already processed the notification.
                    if finished_height >= new.tip().number {
                        debug!(
                            exex_id = %self.id,
                            %notification_id,
                            %finished_height,
                            new_tip = %new.tip().number,
                            "Skipping notification"
                        );

                        self.next_notification_id = notification_id + 1;
                        return Poll::Ready(Ok(()))
                    }
                }
                // Do not handle [ExExNotification::ChainReorged] and
                // [ExExNotification::ChainReverted] cases and always send the
                // notification, because the ExEx should be aware of the reorgs and reverts lower
                // than its finished height
                ExExNotification::ChainReorged { .. } | ExExNotification::ChainReverted { .. } => {}
            }
        }

        debug!(
            exex_id = %self.id,
            %notification_id,
            "Reserving slot for notification"
        );
        match self.sender.poll_reserve(cx) {
            Poll::Ready(Ok(())) => (),
            other => return other,
        }

        debug!(
            exex_id = %self.id,
            %notification_id,
            "Sending notification"
        );
        match self.sender.send_item(notification.clone()) {
            Ok(()) => {
                self.next_notification_id = notification_id + 1;
                self.metrics.notifications_sent_total.increment(1);
                Poll::Ready(Ok(()))
            }
            Err(err) => Poll::Ready(Err(err)),
        }
    }
}

/// Metrics for the `ExEx` manager.
#[derive(Metrics)]
#[metrics(scope = "exex_manager")]
pub struct ExExManagerMetrics {
    /// Max size of the internal state notifications buffer.
    max_capacity: Gauge,
    /// Current capacity of the internal state notifications buffer.
    current_capacity: Gauge,
    /// Current size of the internal state notifications buffer.
    ///
    /// Note that this might be slightly bigger than the maximum capacity in some cases.
    buffer_size: Gauge,
    /// Current number of `ExEx`'s on the node.
    num_exexs: Gauge,
}

/// The execution extension manager.
///
/// The manager is responsible for:
///
/// - Receiving relevant events from the rest of the node, and sending these to the execution
///   extensions
/// - Backpressure
/// - Error handling
/// - Monitoring
#[derive(Debug)]
pub struct ExExManager {
    /// Handles to communicate with the `ExEx`'s.
    exex_handles: Vec<ExExHandle>,

    /// [`ExExNotification`] channel from the [`ExExManagerHandle`]s.
    handle_rx: UnboundedReceiver<ExExNotification>,

    /// The minimum notification ID currently present in the buffer.
    min_id: usize,
    /// Monotonically increasing ID for [`ExExNotification`]s.
    next_id: usize,
    /// Internal buffer of [`ExExNotification`]s.
    ///
    /// The first element of the tuple is a monotonically increasing ID unique to the notification
    /// (the second element of the tuple).
    buffer: VecDeque<(usize, ExExNotification)>,
    /// Max size of the internal state notifications buffer.
    max_capacity: usize,
    /// Current state notifications buffer capacity.
    ///
    /// Used to inform the execution stage of possible batch sizes.
    current_capacity: Arc<AtomicUsize>,

    /// Whether the manager is ready to receive new notifications.
    is_ready: watch::Sender<bool>,

    /// The finished height of all `ExEx`'s.
    finished_height: watch::Sender<FinishedExExHeight>,

    /// A handle to the `ExEx` manager.
    handle: ExExManagerHandle,
    /// Metrics for the `ExEx` manager.
    metrics: ExExManagerMetrics,
}

impl ExExManager {
    /// Create a new [`ExExManager`].
    ///
    /// You must provide an [`ExExHandle`] for each `ExEx` and the maximum capacity of the
    /// notification buffer in the manager.
    ///
    /// When the capacity is exceeded (which can happen if an `ExEx` is slow) no one can send
    /// notifications over [`ExExManagerHandle`]s until there is capacity again.
    pub fn new(handles: Vec<ExExHandle>, max_capacity: usize) -> Self {
        let num_exexs = handles.len();

        let (handle_tx, handle_rx) = mpsc::unbounded_channel();
        let (is_ready_tx, is_ready_rx) = watch::channel(true);
        let (finished_height_tx, finished_height_rx) = watch::channel(if num_exexs == 0 {
            FinishedExExHeight::NoExExs
        } else {
            FinishedExExHeight::NotReady
        });

        let current_capacity = Arc::new(AtomicUsize::new(max_capacity));

        let metrics = ExExManagerMetrics::default();
        metrics.max_capacity.set(max_capacity as f64);
        metrics.num_exexs.set(num_exexs as f64);

        Self {
            exex_handles: handles,

            handle_rx,

            min_id: 0,
            next_id: 0,
            buffer: VecDeque::with_capacity(max_capacity),
            max_capacity,
            current_capacity: Arc::clone(&current_capacity),

            is_ready: is_ready_tx,
            finished_height: finished_height_tx,

            handle: ExExManagerHandle {
                exex_tx: handle_tx,
                num_exexs,
                is_ready_receiver: is_ready_rx.clone(),
                is_ready: ReusableBoxFuture::new(make_wait_future(is_ready_rx)),
                current_capacity,
                finished_height: finished_height_rx,
            },
            metrics,
        }
    }

    /// Returns the handle to the manager.
    pub fn handle(&self) -> ExExManagerHandle {
        self.handle.clone()
    }

    /// Updates the current buffer capacity and notifies all `is_ready` watchers of the manager's
    /// readiness to receive notifications.
    fn update_capacity(&self) {
        let capacity = self.max_capacity.saturating_sub(self.buffer.len());
        self.current_capacity.store(capacity, Ordering::Relaxed);
        self.metrics.current_capacity.set(capacity as f64);
        self.metrics.buffer_size.set(self.buffer.len() as f64);

        // we can safely ignore if the channel is closed, since the manager always holds it open
        // internally
        let _ = self.is_ready.send(capacity > 0);
    }

    /// Pushes a new notification into the managers internal buffer, assigning the notification a
    /// unique ID.
    fn push_notification(&mut self, notification: ExExNotification) {
        let next_id = self.next_id;
        self.buffer.push_back((next_id, notification));
        self.next_id += 1;
    }
}

impl Future for ExExManager {
    type Output = eyre::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // drain handle notifications
        while self.buffer.len() < self.max_capacity {
            if let Poll::Ready(Some(notification)) = self.handle_rx.poll_recv(cx) {
                debug!(
                    committed_tip = ?notification.committed_chain().map(|chain| chain.tip().number),
                    reverted_tip = ?notification.reverted_chain().map(|chain| chain.tip().number),
                    "Received new notification"
                );
                self.push_notification(notification);
                continue
            }
            break
        }

        // update capacity
        self.update_capacity();

        // advance all poll senders
        let mut min_id = usize::MAX;
        for idx in (0..self.exex_handles.len()).rev() {
            let mut exex = self.exex_handles.swap_remove(idx);

            // it is a logic error for this to ever underflow since the manager manages the
            // notification IDs
            let notification_index = exex
                .next_notification_id
                .checked_sub(self.min_id)
                .expect("exex expected notification ID outside the manager's range");
            if let Some(notification) = self.buffer.get(notification_index) {
                if let Poll::Ready(Err(err)) = exex.send(cx, notification) {
                    // the channel was closed, which is irrecoverable for the manager
                    return Poll::Ready(Err(err.into()))
                }
            }
            min_id = min_id.min(exex.next_notification_id);
            self.exex_handles.push(exex);
        }

        // remove processed buffered notifications
        debug!(%min_id, "Updating lowest notification id in buffer");
        self.buffer.retain(|&(id, _)| id >= min_id);
        self.min_id = min_id;

        // update capacity
        self.update_capacity();

        // handle incoming exex events
        for exex in &mut self.exex_handles {
            while let Poll::Ready(Some(event)) = exex.receiver.poll_recv(cx) {
                debug!(exex_id = %exex.id, ?event, "Received event from exex");
                exex.metrics.events_sent_total.increment(1);
                match event {
                    ExExEvent::FinishedHeight(height) => exex.finished_height = Some(height),
                }
            }
        }

        // update watch channel block number
        let finished_height = self.exex_handles.iter_mut().try_fold(u64::MAX, |curr, exex| {
            exex.finished_height.map_or(Err(()), |height| Ok(height.min(curr)))
        });
        if let Ok(finished_height) = finished_height {
            let _ = self.finished_height.send(FinishedExExHeight::Height(finished_height));
        }

        Poll::Pending
    }
}

/// A handle to communicate with the [`ExExManager`].
#[derive(Debug)]
pub struct ExExManagerHandle {
    /// Channel to send notifications to the `ExEx` manager.
    exex_tx: UnboundedSender<ExExNotification>,
    /// The number of `ExEx`'s running on the node.
    num_exexs: usize,
    /// A watch channel denoting whether the manager is ready for new notifications or not.
    ///
    /// This is stored internally alongside a `ReusableBoxFuture` representation of the same value.
    /// This field is only used to create a new `ReusableBoxFuture` when the handle is cloned,
    /// but is otherwise unused.
    is_ready_receiver: watch::Receiver<bool>,
    /// A reusable future that resolves when the manager is ready for new
    /// notifications.
    is_ready: ReusableBoxFuture<'static, watch::Receiver<bool>>,
    /// The current capacity of the manager's internal notification buffer.
    current_capacity: Arc<AtomicUsize>,
    /// The finished height of all `ExEx`'s.
    finished_height: watch::Receiver<FinishedExExHeight>,
}

impl ExExManagerHandle {
    /// Creates an empty manager handle.
    ///
    /// Use this if there is no manager present.
    ///
    /// The handle will always be ready, and have a capacity of 0.
    pub fn empty() -> Self {
        let (exex_tx, _) = mpsc::unbounded_channel();
        let (_, is_ready_rx) = watch::channel(true);
        let (_, finished_height_rx) = watch::channel(FinishedExExHeight::NoExExs);

        Self {
            exex_tx,
            num_exexs: 0,
            is_ready_receiver: is_ready_rx.clone(),
            is_ready: ReusableBoxFuture::new(make_wait_future(is_ready_rx)),
            current_capacity: Arc::new(AtomicUsize::new(0)),
            finished_height: finished_height_rx,
        }
    }

    /// Synchronously send a notification over the channel to all execution extensions.
    ///
    /// Senders should call [`Self::has_capacity`] first.
    pub fn send(&self, notification: ExExNotification) -> Result<(), SendError<ExExNotification>> {
        self.exex_tx.send(notification)
    }

    /// Asynchronously send a notification over the channel to all execution extensions.
    ///
    /// The returned future resolves when the notification has been delivered. If there is no
    /// capacity in the channel, the future will wait.
    pub async fn send_async(
        &mut self,
        notification: ExExNotification,
    ) -> Result<(), SendError<ExExNotification>> {
        self.ready().await;
        self.exex_tx.send(notification)
    }

    /// Get the current capacity of the `ExEx` manager's internal notification buffer.
    pub fn capacity(&self) -> usize {
        self.current_capacity.load(Ordering::Relaxed)
    }

    /// Whether there is capacity in the `ExEx` manager's internal notification buffer.
    ///
    /// If this returns `false`, the owner of the handle should **NOT** send new notifications over
    /// the channel until the manager is ready again, as this can lead to unbounded memory growth.
    pub fn has_capacity(&self) -> bool {
        self.capacity() > 0
    }

    /// Returns `true` if there are `ExEx`'s installed in the node.
    pub const fn has_exexs(&self) -> bool {
        self.num_exexs > 0
    }

    /// The finished height of all `ExEx`'s.
    pub fn finished_height(&self) -> watch::Receiver<FinishedExExHeight> {
        self.finished_height.clone()
    }

    /// Wait until the manager is ready for new notifications.
    pub async fn ready(&mut self) {
        poll_fn(|cx| self.poll_ready(cx)).await
    }

    /// Wait until the manager is ready for new notifications.
    pub fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let rx = ready!(self.is_ready.poll(cx));
        self.is_ready.set(make_wait_future(rx));
        Poll::Ready(())
    }
}

/// Creates a future that resolves once the given watch channel receiver is true.
async fn make_wait_future(mut rx: watch::Receiver<bool>) -> watch::Receiver<bool> {
    // NOTE(onbjerg): We can ignore the error here, because if the channel is closed, the node
    // is shutting down.
    let _ = rx.wait_for(|ready| *ready).await;
    rx
}

impl Clone for ExExManagerHandle {
    fn clone(&self) -> Self {
        Self {
            exex_tx: self.exex_tx.clone(),
            num_exexs: self.num_exexs,
            is_ready_receiver: self.is_ready_receiver.clone(),
            is_ready: ReusableBoxFuture::new(make_wait_future(self.is_ready_receiver.clone())),
            current_capacity: self.current_capacity.clone(),
            finished_height: self.finished_height.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::{SealedBlockWithSenders, B256};
    use reth_provider::Chain;

    #[tokio::test]
    async fn test_delivers_events() {
        let (mut exex_handle, event_tx, mut _notification_rx) =
            ExExHandle::new("test_exex".to_string());

        // Send an event and check that it's delivered correctly
        event_tx.send(ExExEvent::FinishedHeight(42)).unwrap();
        let received_event = exex_handle.receiver.recv().await.unwrap();
        assert_eq!(received_event, ExExEvent::FinishedHeight(42));
    }

    #[tokio::test]
    async fn test_has_exexs() {
        let (exex_handle_1, _, _) = ExExHandle::new("test_exex_1".to_string());

        assert!(!ExExManager::new(vec![], 0).handle.has_exexs());

        assert!(ExExManager::new(vec![exex_handle_1], 0).handle.has_exexs());
    }

    #[tokio::test]
    async fn test_has_capacity() {
        let (exex_handle_1, _, _) = ExExHandle::new("test_exex_1".to_string());

        assert!(!ExExManager::new(vec![], 0).handle.has_capacity());

        assert!(ExExManager::new(vec![exex_handle_1], 10).handle.has_capacity());
    }

    #[test]
    fn test_push_notification() {
        let (exex_handle, _, _) = ExExHandle::new("test_exex".to_string());

        // Create a mock ExExManager and add the exex_handle to it
        let mut exex_manager = ExExManager::new(vec![exex_handle], 10);

        // Define the notification for testing
        let mut block1 = SealedBlockWithSenders::default();
        block1.block.header.set_hash(B256::new([0x01; 32]));
        block1.block.header.set_block_number(10);

        let notification1 = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(vec![block1.clone()], Default::default(), Default::default())),
        };

        // Push the first notification
        exex_manager.push_notification(notification1.clone());

        // Verify the buffer contains the notification with the correct ID
        assert_eq!(exex_manager.buffer.len(), 1);
        assert_eq!(exex_manager.buffer.front().unwrap().0, 0);
        assert_eq!(exex_manager.buffer.front().unwrap().1, notification1);
        assert_eq!(exex_manager.next_id, 1);

        // Push another notification
        let mut block2 = SealedBlockWithSenders::default();
        block2.block.header.set_hash(B256::new([0x02; 32]));
        block2.block.header.set_block_number(20);

        let notification2 = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(vec![block2.clone()], Default::default(), Default::default())),
        };

        exex_manager.push_notification(notification2.clone());

        // Verify the buffer contains both notifications with correct IDs
        assert_eq!(exex_manager.buffer.len(), 2);
        assert_eq!(exex_manager.buffer.front().unwrap().0, 0);
        assert_eq!(exex_manager.buffer.front().unwrap().1, notification1);
        assert_eq!(exex_manager.buffer.get(1).unwrap().0, 1);
        assert_eq!(exex_manager.buffer.get(1).unwrap().1, notification2);
        assert_eq!(exex_manager.next_id, 2);
    }

    #[test]
    fn test_update_capacity() {
        let (exex_handle, _, _) = ExExHandle::new("test_exex".to_string());

        // Create a mock ExExManager and add the exex_handle to it
        let max_capacity = 5;
        let mut exex_manager = ExExManager::new(vec![exex_handle], max_capacity);

        // Push some notifications to fill part of the buffer
        let mut block1 = SealedBlockWithSenders::default();
        block1.block.header.set_hash(B256::new([0x01; 32]));
        block1.block.header.set_block_number(10);

        let notification1 = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(vec![block1.clone()], Default::default(), Default::default())),
        };

        exex_manager.push_notification(notification1.clone());
        exex_manager.push_notification(notification1);

        // Update capacity
        exex_manager.update_capacity();

        // Verify current capacity and metrics
        assert_eq!(exex_manager.current_capacity.load(Ordering::Relaxed), max_capacity - 2);

        // Clear the buffer and update capacity
        exex_manager.buffer.clear();
        exex_manager.update_capacity();

        // Verify current capacity
        assert_eq!(exex_manager.current_capacity.load(Ordering::Relaxed), max_capacity);
    }

    #[tokio::test]
    async fn test_updates_block_height() {
        let (exex_handle, event_tx, mut _notification_rx) =
            ExExHandle::new("test_exex".to_string());

        // Check initial block height
        assert!(exex_handle.finished_height.is_none());

        // Update the block height via an event
        event_tx.send(ExExEvent::FinishedHeight(42)).unwrap();

        // Create a mock ExExManager and add the exex_handle to it
        let exex_manager = ExExManager::new(vec![exex_handle], 10);

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        // Pin the ExExManager to call the poll method
        let mut pinned_manager = std::pin::pin!(exex_manager);
        let _ = pinned_manager.as_mut().poll(&mut cx);

        // Check that the block height was updated
        let updated_exex_handle = &pinned_manager.exex_handles[0];
        assert_eq!(updated_exex_handle.finished_height, Some(42));

        // Get the receiver for the finished height
        let mut receiver = pinned_manager.handle.finished_height();

        // Wait for a new value to be sent
        receiver.changed().await.unwrap();

        // Get the latest value
        let finished_height = *receiver.borrow();

        // The finished height should be updated to the lower block height
        assert_eq!(finished_height, FinishedExExHeight::Height(42));
    }

    #[tokio::test]
    async fn test_updates_block_height_lower() {
        // Create two `ExExHandle` instances
        let (exex_handle1, event_tx1, _) = ExExHandle::new("test_exex1".to_string());
        let (exex_handle2, event_tx2, _) = ExExHandle::new("test_exex2".to_string());

        // Send events to update the block heights of the two handles, with the second being lower
        event_tx1.send(ExExEvent::FinishedHeight(42)).unwrap();
        event_tx2.send(ExExEvent::FinishedHeight(10)).unwrap();

        let exex_manager = ExExManager::new(vec![exex_handle1, exex_handle2], 10);

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        let mut pinned_manager = std::pin::pin!(exex_manager);

        let _ = pinned_manager.as_mut().poll(&mut cx);

        // Get the receiver for the finished height
        let mut receiver = pinned_manager.handle.finished_height();

        // Wait for a new value to be sent
        receiver.changed().await.unwrap();

        // Get the latest value
        let finished_height = *receiver.borrow();

        // The finished height should be updated to the lower block height
        assert_eq!(finished_height, FinishedExExHeight::Height(10));
    }

    #[tokio::test]
    async fn test_updates_block_height_greater() {
        // Create two `ExExHandle` instances
        let (exex_handle1, event_tx1, _) = ExExHandle::new("test_exex1".to_string());
        let (exex_handle2, event_tx2, _) = ExExHandle::new("test_exex2".to_string());

        // Assert that the initial block height is `None` for the first `ExExHandle`.
        assert!(exex_handle1.finished_height.is_none());

        // Send events to update the block heights of the two handles, with the second being higher.
        event_tx1.send(ExExEvent::FinishedHeight(42)).unwrap();
        event_tx2.send(ExExEvent::FinishedHeight(100)).unwrap();

        let exex_manager = ExExManager::new(vec![exex_handle1, exex_handle2], 10);

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        let mut pinned_manager = std::pin::pin!(exex_manager);

        let _ = pinned_manager.as_mut().poll(&mut cx);

        // Get the receiver for the finished height
        let mut receiver = pinned_manager.handle.finished_height();

        // Wait for a new value to be sent
        receiver.changed().await.unwrap();

        // Get the latest value
        let finished_height = *receiver.borrow();

        // The finished height should be updated to the lower block height
        assert_eq!(finished_height, FinishedExExHeight::Height(42));

        // // The lower block height should be retained
        // let updated_exex_handle = &pinned_manager.exex_handles[0];
        // assert_eq!(updated_exex_handle.finished_height, Some(42));
    }

    #[tokio::test]
    async fn test_exex_manager_capacity() {
        let (exex_handle_1, _, _) = ExExHandle::new("test_exex_1".to_string());

        // Create an ExExManager with a small max capacity
        let max_capacity = 2;
        let mut exex_manager = ExExManager::new(vec![exex_handle_1], max_capacity);

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        // Setup a notification
        let notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![Default::default()],
                Default::default(),
                Default::default(),
            )),
        };

        // Send notifications to go over the max capacity
        exex_manager.handle.exex_tx.send(notification.clone()).unwrap();
        exex_manager.handle.exex_tx.send(notification.clone()).unwrap();
        exex_manager.handle.exex_tx.send(notification).unwrap();

        // Pin the ExExManager to call the poll method
        let mut pinned_manager = std::pin::pin!(exex_manager);

        // Before polling, the next notification ID should be 0 and the buffer should be empty
        assert_eq!(pinned_manager.next_id, 0);
        assert_eq!(pinned_manager.buffer.len(), 0);

        let _ = pinned_manager.as_mut().poll(&mut cx);

        // After polling, the next notification ID and buffer size should be updated
        assert_eq!(pinned_manager.next_id, 2);
        assert_eq!(pinned_manager.buffer.len(), 2);
    }

    #[tokio::test]
    async fn exex_handle_new() {
        let (mut exex_handle, _, mut notification_rx) = ExExHandle::new("test_exex".to_string());

        // Check initial state
        assert_eq!(exex_handle.id, "test_exex");
        assert_eq!(exex_handle.next_notification_id, 0);

        // Setup two blocks for the chain commit notification
        let mut block1 = SealedBlockWithSenders::default();
        block1.block.header.set_hash(B256::new([0x01; 32]));
        block1.block.header.set_block_number(10);

        let mut block2 = SealedBlockWithSenders::default();
        block2.block.header.set_hash(B256::new([0x02; 32]));
        block2.block.header.set_block_number(11);

        // Setup a notification
        let notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![block1.clone(), block2.clone()],
                Default::default(),
                Default::default(),
            )),
        };

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        // Send a notification and ensure it's received correctly
        match exex_handle.send(&mut cx, &(22, notification.clone())) {
            Poll::Ready(Ok(())) => {
                let received_notification = notification_rx.recv().await.unwrap();
                assert_eq!(received_notification, notification);
            }
            Poll::Pending => panic!("Notification send is pending"),
            Poll::Ready(Err(e)) => panic!("Failed to send notification: {:?}", e),
        }

        // Ensure the notification ID was incremented
        assert_eq!(exex_handle.next_notification_id, 23);
    }

    #[tokio::test]
    async fn test_notification_if_finished_height_gt_chain_tip() {
        let (mut exex_handle, _, mut notification_rx) = ExExHandle::new("test_exex".to_string());

        // Set finished_height to a value higher than the block tip
        exex_handle.finished_height = Some(15);

        let mut block1 = SealedBlockWithSenders::default();
        block1.block.header.set_hash(B256::new([0x01; 32]));
        block1.block.header.set_block_number(10);

        let notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(vec![block1.clone()], Default::default(), Default::default())),
        };

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        // Send the notification
        match exex_handle.send(&mut cx, &(22, notification)) {
            Poll::Ready(Ok(())) => {
                // The notification should be skipped, so nothing should be sent.
                // Check that the receiver channel is indeed empty
                assert!(notification_rx.try_recv().is_err(), "Receiver channel should be empty");
            }
            Poll::Pending | Poll::Ready(Err(_)) => {
                panic!("Notification should not be pending or fail");
            }
        }

        // Ensure the notification ID was still incremented
        assert_eq!(exex_handle.next_notification_id, 23);
    }

    #[tokio::test]
    async fn test_sends_chain_reorged_notification() {
        let (mut exex_handle, _, mut notification_rx) = ExExHandle::new("test_exex".to_string());

        let notification = ExExNotification::ChainReorged {
            old: Arc::new(Chain::default()),
            new: Arc::new(Chain::default()),
        };

        // Even if the finished height is higher than the tip of the new chain, the reorg
        // notification should be received
        exex_handle.finished_height = Some(u64::MAX);

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        // Send the notification
        match exex_handle.send(&mut cx, &(22, notification.clone())) {
            Poll::Ready(Ok(())) => {
                let received_notification = notification_rx.recv().await.unwrap();
                assert_eq!(received_notification, notification);
            }
            Poll::Pending | Poll::Ready(Err(_)) => {
                panic!("Notification should not be pending or fail")
            }
        }

        // Ensure the notification ID was incremented
        assert_eq!(exex_handle.next_notification_id, 23);
    }

    #[tokio::test]
    async fn test_sends_chain_reverted_notification() {
        let (mut exex_handle, _, mut notification_rx) = ExExHandle::new("test_exex".to_string());

        let notification = ExExNotification::ChainReverted { old: Arc::new(Chain::default()) };

        // Even if the finished height is higher than the tip of the new chain, the reorg
        // notification should be received
        exex_handle.finished_height = Some(u64::MAX);

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        // Send the notification
        match exex_handle.send(&mut cx, &(22, notification.clone())) {
            Poll::Ready(Ok(())) => {
                let received_notification = notification_rx.recv().await.unwrap();
                assert_eq!(received_notification, notification);
            }
            Poll::Pending | Poll::Ready(Err(_)) => {
                panic!("Notification should not be pending or fail")
            }
        }

        // Ensure the notification ID was incremented
        assert_eq!(exex_handle.next_notification_id, 23);
    }
}
