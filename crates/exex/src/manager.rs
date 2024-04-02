use crate::{ExExEvent, ExExNotification};
use metrics::Gauge;
use reth_metrics::{metrics::Counter, Metrics};
use reth_primitives::{BlockNumber, FinishedExExHeight};
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

/// Metrics for an ExEx.
#[derive(Metrics)]
#[metrics(scope = "exex")]
struct ExExMetrics {
    /// The total number of notifications sent to an ExEx.
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

    /// Channel to send [`ExExNotification`]s to the ExEx.
    sender: PollSender<ExExNotification>,
    /// Channel to receive [`ExExEvent`]s from the ExEx.
    receiver: UnboundedReceiver<ExExEvent>,
    /// The ID of the next notification to send to this ExEx.
    next_notification_id: usize,

    /// The finished block number of the ExEx.
    ///
    /// If this is `None`, the ExEx has not emitted a `FinishedHeight` event.
    finished_height: Option<BlockNumber>,
}

impl ExExHandle {
    /// Create a new handle for the given ExEx.
    ///
    /// Returns the handle, as well as a [`UnboundedSender`] for [`ExExEvent`]s and a
    /// [`Receiver`] for [`ExExNotification`]s that should be given to the ExEx.
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

/// Metrics for the ExEx manager.
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
    /// Current number of ExEx's on the node.
    num_exexs: Gauge,
}

/// The execution extension manager.
///
/// The manager is responsible for:
///
/// - Receiving relevant events from the rest of the node, and sending these to the execution
/// extensions
/// - Backpressure
/// - Error handling
/// - Monitoring
#[derive(Debug)]
pub struct ExExManager {
    /// Handles to communicate with the ExEx's.
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

    /// The finished height of all ExEx's.
    finished_height: watch::Sender<FinishedExExHeight>,

    /// A handle to the ExEx manager.
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
        for exex in self.exex_handles.iter_mut() {
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
            let _ = self.finished_height.send(FinishedExExHeight::Height(finished_height));
        }

        Poll::Pending
    }
}

/// A handle to communicate with the [`ExExManager`].
#[derive(Debug)]
pub struct ExExManagerHandle {
    /// Channel to send notifications to the ExEx manager.
    exex_tx: UnboundedSender<ExExNotification>,
    /// The number of ExEx's running on the node.
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
    /// The finished height of all ExEx's.
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
