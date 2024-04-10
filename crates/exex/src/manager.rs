use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

use crate::ExExEvent;
use metrics::Gauge;
use reth_metrics::{metrics::Counter, Metrics};
use reth_primitives::BlockNumber;
use reth_provider::CanonStateNotification;
use tokio::sync::{
    mpsc::{self, error::SendError, Receiver, UnboundedReceiver, UnboundedSender},
    watch,
};
use tokio_util::sync::{PollSendError, PollSender};

/// Metrics for an ExEx.
#[derive(Metrics)]
#[metrics(scope = "exex")]
struct ExExMetrics {
    /// The number of canonical state notifications processed by an ExEx.
    notifications_processed: Counter,
    /// The number of events an ExEx has sent to the manager.
    events_sent: Counter,
}

/// A handle to an ExEx used by the [`ExExManager`] to communicate with ExEx's.
///
/// A handle should be created for each ExEx with a unique ID. The channels returned by
/// [`ExExHandle::new`] should be given to the ExEx, while the handle itself should be given to the
/// manager in [`ExExManager::new`].
#[derive(Debug)]
pub struct ExExHandle {
    /// The execution extension's ID.
    id: String,
    /// Metrics for an ExEx.
    metrics: ExExMetrics,

    /// Channel to send [`CanonStateNotification`]s to the ExEx.
    sender: PollSender<CanonStateNotification>,
    /// Channel to receive [`ExExEvent`]s from the ExEx.
    receiver: UnboundedReceiver<ExExEvent>,
    /// The ID of the next notification to send to this ExEx.
    next_notification_id: usize,

    /// The finished block number of the ExEx.
    /// tbd describe how this can change, what None means, what this is used for
    finished_height: Option<BlockNumber>,
}

impl ExExHandle {
    /// Create a new handle for the given ExEx.
    ///
    /// Returns the handle, as well as a [`Sender`] for [`ExExEvent`]s and a
    /// [`Receiver`] for [`CanonStateNotification`]s that should be given to the ExEx.
    pub fn new(id: String) -> (Self, UnboundedSender<ExExEvent>, Receiver<CanonStateNotification>) {
        let (canon_tx, canon_rx) = mpsc::channel(1);
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        (
            Self {
                id: id.clone(),
                metrics: ExExMetrics::new_with_labels(&[("exex", id)]),
                sender: PollSender::new(canon_tx),
                receiver: event_rx,
                next_notification_id: 0,
                finished_height: None,
            },
            event_tx,
            canon_rx,
        )
    }

    /// Reserves a slot in the `PollSender` channel and sends the notification if the slot was
    /// successfully reserved.
    ///
    /// When the notification is sent, it is considered delivered.
    fn send(
        &mut self,
        cx: &mut Context<'_>,
        (event_id, notification): &(usize, CanonStateNotification),
    ) -> Poll<Result<(), PollSendError<CanonStateNotification>>> {
        match self.sender.poll_reserve(cx) {
            Poll::Ready(Ok(())) => (),
            other => return other,
        }

        match self.sender.send_item(notification.clone()) {
            Ok(()) => {
                self.next_notification_id = event_id + 1;
                self.metrics.notifications_processed.increment(1);
                Poll::Ready(Ok(()))
            }
            Err(err) => Poll::Ready(Err(err)),
        }
    }
}

// todo
#[derive(Metrics)]
#[metrics(scope = "exex_manager")]
pub struct ExExManagerMetrics {
    /// Max size of the internal state notifications buffer.
    max_capacity: Counter,
    /// Current capacity of the internal state notifications buffer.
    current_capacity: Gauge,
    /// Current size of the internal state notifications buffer.
    ///
    /// Note that this might be slightly bigger than the maximum capacity in some cases.
    buffer_size: Gauge,
}

// todo: if event is sent to exex it is considered delivered
/// The execution extension manager.
///
/// The manager is responsible for:
///
/// - Receiving relevant events from the rest of the node, and sending these to the execution
/// extensions
/// - Backpressure
/// - Error handling
/// - Monitoring
///
/// TBD
#[derive(Debug)]
pub struct ExExManager {
    /// Handles to communicate with the ExEx's.
    // todo(onbjerg): we should document that these notifications can include blocks the exex does
    // not care about if a longer chain segment is sent - filtering is up to the exex. where do
    // we document it, though?
    exex_handles: Vec<ExExHandle>,

    /// [`CanonStateNotification`] channel from the [`ExExManagerHandle`]s.
    handle_rx: UnboundedReceiver<CanonStateNotification>,

    /// The minimum notification ID currently present in the buffer.
    min_id: usize,
    /// Monotonically increasing ID for [`CanonStateNotification`]s.
    next_id: usize,
    /// Internal buffer of [`CanonStateNotification`]s.
    buffer: VecDeque<(usize, CanonStateNotification)>,
    /// Max size of the internal state notifications buffer.
    max_capacity: usize,
    /// Current state notifications buffer capacity.
    ///
    /// Used to inform the execution stage of possible batch sizes.
    current_capacity: Arc<AtomicUsize>,

    /// Whether the manager is ready to receive new notifications.
    is_ready: watch::Sender<bool>,

    /// block number for pruner/exec stage (tbd)
    /// todo(onbjerg): this is inclusive, note that in exex too, maybe rename FinishedHeight
    block: watch::Sender<BlockNumber>,

    /// tbd
    handle: ExExManagerHandle,
    /// Metrics for the ExEx manager.
    metrics: ExExManagerMetrics,
}

impl ExExManager {
    /// Create a new [`ExExManager`].
    ///
    /// You must provide an [`ExExHandle`] for each ExEx and the maximum capacity of the
    /// notification buffer in the manager.
    ///
    /// When the capacity is exceeded (which can happen if an ExEx is slow) no one can send
    /// messages over [`ExExManagerHandle`]s until there is capacity again.
    pub fn new(handles: Vec<ExExHandle>, max_capacity: usize) -> Self {
        let num_exexs = handles.len();

        let (handle_tx, handle_rx) = mpsc::unbounded_channel();
        let (is_ready_tx, is_ready_rx) = watch::channel(true);
        let (block_tx, block_rx) = watch::channel(0);

        let current_capacity = Arc::new(AtomicUsize::new(max_capacity));

        let metrics = ExExManagerMetrics::default();
        metrics.max_capacity.absolute(max_capacity as u64);

        Self {
            exex_handles: handles,

            handle_rx,

            min_id: 0,
            next_id: 0,
            buffer: VecDeque::with_capacity(max_capacity),
            max_capacity,
            current_capacity: Arc::clone(&current_capacity),

            is_ready: is_ready_tx,
            block: block_tx,

            handle: ExExManagerHandle {
                exex_tx: handle_tx,
                num_exexs,
                is_ready: is_ready_rx,
                current_capacity,
                block: block_rx,
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
    fn update_capacity(&mut self) {
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
    fn push_notification(&mut self, notification: CanonStateNotification) {
        let next_id = self.next_id;
        self.buffer.push_back((next_id, notification));
        self.next_id += 1;
    }
}

impl Future for ExExManager {
    // todo
    type Output = Result<(), ()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // drain handle notifications
        'notifications: while self.buffer.len() < self.max_capacity {
            if let Poll::Ready(Some(notification)) = self.handle_rx.poll_recv(cx) {
                self.push_notification(notification);
                continue 'notifications
            }
            break
        }

        // todo: drain blockchain tree notifications
        // update capacity
        self.update_capacity();

        // advance all poll senders
        let mut min_id = usize::MAX;
        for idx in (0..self.exex_handles.len()).rev() {
            let mut exex = self.exex_handles.swap_remove(idx);

            // it is a logic error for this to ever underflow since the manager manages the
            // notification IDs
            let notification_id = exex
                .next_notification_id
                .checked_sub(self.min_id)
                .expect("exex expected notification ID outside the manager's range");
            if let Some(notification) = self.buffer.get(notification_id) {
                if let Poll::Ready(Err(_)) = exex.send(cx, notification) {
                    // the channel was closed, which is irrecoverable for the manager
                    return Poll::Ready(Err(()))
                }
            }
            min_id = min_id.min(exex.next_notification_id);
            self.exex_handles.push(exex);
        }

        // remove processed buffered events
        self.buffer.retain(|&(id, _)| id > min_id);
        self.min_id = min_id;

        // update capacity
        self.update_capacity();

        // handle incoming exex events
        for exex in self.exex_handles.iter_mut() {
            while let Poll::Ready(Some(event)) = exex.receiver.poll_recv(cx) {
                match event {
                    ExExEvent::FinishedHeight(height) => exex.finished_height = Some(height),
                }
            }
        }

        // update watch channel block number
        // todo: clean this up and also is this too expensive
        let finished_height = self.exex_handles.iter_mut().try_fold(u64::MAX, |curr, exex| {
            let height = match exex.finished_height {
                None => return Err(()),
                Some(height) => height,
            };

            if height < curr {
                Ok(height)
            } else {
                Ok(curr)
            }
        });
        if let Ok(finished_height) = finished_height {
            let _ = self.block.send(finished_height);
        }

        Poll::Pending
    }
}

/// TBD
#[derive(Debug, Clone)]
pub struct ExExManagerHandle {
    exex_tx: UnboundedSender<CanonStateNotification>,
    num_exexs: usize,
    is_ready: watch::Receiver<bool>,
    current_capacity: Arc<AtomicUsize>,
    block: watch::Receiver<BlockNumber>,
}

impl ExExManagerHandle {
    /// Whether we should send a notification for a given block number.
    ///
    /// This checks that:
    ///
    /// - The block number is interesting to at least one ExEx.
    /// - That the manager has capacity in its internal buffer for the notification
    /// - That there are any ExEx's currently running
    ///
    /// For [`CanonStateNotification`]s with more than one block, pass the highest block in the
    /// chain.
    pub fn should_send(&mut self, block_number: BlockNumber) -> bool {
        let has_exexs = self.num_exexs > 0;
        let within_threshold = block_number >= *self.block.borrow_and_update();

        has_exexs && within_threshold && self.has_capacity()
    }

    /// Send a notification over the channel to all execution extensions.
    ///
    /// Senders should call [`should_send`] first.
    pub fn send(
        &self,
        notification: CanonStateNotification,
    ) -> Result<(), SendError<CanonStateNotification>> {
        self.exex_tx.send(notification)
    }

    /// Send a notification over the channel to all execution extensions.
    ///
    /// The returned future resolves when the notification has been delivered. If there is no
    /// capacity in the channel, the future will wait.
    pub async fn send_async(
        &mut self,
        notification: CanonStateNotification,
    ) -> Result<(), SendError<CanonStateNotification>> {
        self.ready().await;
        self.exex_tx.send(notification)
    }

    /// Whether there is capacity in the ExEx manager's internal notification buffer.
    ///
    /// If this returns `false`, the owner of the handle should **NOT** send new notifications over
    /// the channel until the manager is ready again, as this can lead to unbounded memory growth.
    pub fn has_capacity(&self) -> bool {
        self.current_capacity.load(Ordering::Relaxed) > 0
    }

    /// Wait until the manager is ready for new notifications.
    pub async fn ready(&mut self) {
        let _ = self.is_ready.wait_for(|val| *val).await;
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn delivers_events() {}

    #[tokio::test]
    async fn capacity() {}

    #[tokio::test]
    async fn updates_block_height() {}

    #[tokio::test]
    async fn slow_exex() {}

    #[tokio::test]
    async fn is_ready() {}
}
