use std::{
    collections::VecDeque,
    future::{poll_fn, Future},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

use crate::ExExEvent;
use futures::StreamExt;
use metrics::Gauge;
use reth_metrics::{metrics::Counter, Metrics};
use reth_primitives::BlockNumber;
use reth_provider::CanonStateNotification;
use reth_tracing::tracing::debug;
use tokio::sync::{
    mpsc::{self, error::SendError, Receiver, UnboundedReceiver, UnboundedSender},
    watch,
};
use tokio_stream::wrappers::WatchStream;
use tokio_util::sync::{PollSendError, PollSender};

/// Metrics for an ExEx.
#[derive(Metrics)]
#[metrics(scope = "exex")]
struct ExExMetrics {
    /// The total number of canonical state notifications sent to an ExEx.
    notifications_sent_total: Counter,
    /// The total number of events an ExEx has sent to the manager.
    events_sent_total: Counter,
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
    /// Returns the handle, as well as a [`UnboundedSender`] for [`ExExEvent`]s and a
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
        // check that this notification is above the finished height of the exex if the exex has set
        // one
        if let Some(finished_height) = self.finished_height {
            if finished_height >= notification.tip().number {
                self.next_notification_id = event_id + 1;
                return Poll::Ready(Ok(()))
            }
        }

        match self.sender.poll_reserve(cx) {
            Poll::Ready(Ok(())) => (),
            other => return other,
        }

        match self.sender.send_item(notification.clone()) {
            Ok(()) => {
                self.next_notification_id = event_id + 1;
                self.metrics.notifications_sent_total.increment(1);
                Poll::Ready(Ok(()))
            }
            Err(err) => Poll::Ready(Err(err)),
        }
    }
}

/// Metrics for the ExEx manager.
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

    /// The finished height of all ExEx's.
    ///
    /// This is the lowest common denominator between all ExEx's. If an ExEx has not emitted a
    /// `FinishedHeight` event, it will be `None`.
    ///
    /// This block is used to (amongst other things) determine what blocks are safe to prune.
    ///
    /// The number is inclusive, i.e. all blocks `<= finished_height` are safe to prune.
    finished_height: watch::Sender<Option<BlockNumber>>,

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
        let (finished_height_tx, finished_height_rx) = watch::channel(None);

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
            finished_height: finished_height_tx,

            handle: ExExManagerHandle {
                exex_tx: handle_tx,
                num_exexs,
                is_ready_receiver: is_ready_rx.clone(),
                is_ready: WatchStream::new(is_ready_rx),
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
                debug!("received new notification");
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
                debug!(exex.id, notification_id, "sent notification to exex");
                if let Poll::Ready(Err(_)) = exex.send(cx, notification) {
                    // the channel was closed, which is irrecoverable for the manager
                    return Poll::Ready(Err(()))
                }
            }
            min_id = min_id.min(exex.next_notification_id);
            self.exex_handles.push(exex);
        }

        // remove processed buffered events
        self.buffer.retain(|&(id, _)| id >= min_id);
        self.min_id = min_id;
        debug!("lowest notification id in buffer is {min_id}");

        // update capacity
        self.update_capacity();

        // handle incoming exex events
        for exex in self.exex_handles.iter_mut() {
            while let Poll::Ready(Some(event)) = exex.receiver.poll_recv(cx) {
                debug!(?event, "received event from exex {}", exex.id);
                exex.metrics.events_sent_total.increment(1);
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
            let _ = self.finished_height.send(Some(finished_height));
        }

        Poll::Pending
    }
}

/// A handle to communicate with the [`ExExManager`].
#[derive(Debug)]
pub struct ExExManagerHandle {
    exex_tx: UnboundedSender<CanonStateNotification>,
    num_exexs: usize,
    is_ready_receiver: watch::Receiver<bool>,
    is_ready: WatchStream<bool>,
    current_capacity: Arc<AtomicUsize>,
    finished_height: watch::Receiver<Option<BlockNumber>>,
}

impl ExExManagerHandle {
    /// Synchronously send a notification over the channel to all execution extensions.
    ///
    /// Senders should call [`Self::has_capacity`] first.
    pub fn send(
        &self,
        notification: CanonStateNotification,
    ) -> Result<(), SendError<CanonStateNotification>> {
        self.exex_tx.send(notification)
    }

    /// Asynchronously send a notification over the channel to all execution extensions.
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

    /// Get the current capacity of the ExEx manager's internal notification buffer.
    pub fn capacity(&self) -> usize {
        self.current_capacity.load(Ordering::Relaxed)
    }

    /// Whether there is capacity in the ExEx manager's internal notification buffer.
    ///
    /// If this returns `false`, the owner of the handle should **NOT** send new notifications over
    /// the channel until the manager is ready again, as this can lead to unbounded memory growth.
    pub fn has_capacity(&self) -> bool {
        self.current_capacity.load(Ordering::Relaxed) > 0
    }

    /// Returns `true` if there are ExEx's installed in the node.
    pub fn has_exexs(&self) -> bool {
        self.num_exexs > 0
    }

    /// The finished height of all ExEx's.
    ///
    /// This is the lowest common denominator between all ExEx's. If an ExEx has not emitted a
    /// `FinishedHeight` event, it will be `None`.
    ///
    /// This block is used to (amongst other things) determine what blocks are safe to prune.
    ///
    /// The number is inclusive, i.e. all blocks `<= finished_height` are safe to prune.
    pub fn finished_height(&mut self) -> Option<BlockNumber> {
        *self.finished_height.borrow_and_update()
    }

    /// Wait until the manager is ready for new notifications.
    pub async fn ready(&mut self) {
        poll_fn(|cx| self.poll_ready(cx)).await
    }

    /// Wait until the manager is ready for new notifications.
    pub fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        // if this returns `Poll::Ready(None)` the stream is exhausted, which means the underlying
        // channel is closed.
        //
        // this can only happen if the manager died, and the node is shutting down, so we ignore it
        let mut pinned = std::pin::pin!(&mut self.is_ready);
        if pinned.poll_next_unpin(cx) == Poll::Ready(Some(true)) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl Clone for ExExManagerHandle {
    fn clone(&self) -> Self {
        Self {
            exex_tx: self.exex_tx.clone(),
            num_exexs: self.num_exexs,
            is_ready_receiver: self.is_ready_receiver.clone(),
            is_ready: WatchStream::new(self.is_ready_receiver.clone()),
            current_capacity: self.current_capacity.clone(),
            finished_height: self.finished_height.clone(),
        }
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
