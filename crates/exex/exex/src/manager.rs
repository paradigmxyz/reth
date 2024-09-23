use crate::{
    BackfillJobFactory, ExExEvent, ExExNotification, FinishedExExHeight, StreamBackfillJob,
};
use alloy_primitives::{BlockNumber, U256};
use eyre::OptionExt;
use futures::{Stream, StreamExt};
use metrics::Gauge;
use reth_chainspec::Head;
use reth_evm::execute::BlockExecutorProvider;
use reth_exex_types::ExExHead;
use reth_metrics::{metrics::Counter, Metrics};
use reth_provider::{BlockReader, Chain, HeaderProvider, StateProviderFactory};
use reth_tracing::tracing::debug;
use std::{
    collections::VecDeque,
    fmt::Debug,
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
    pub fn new<P, E>(
        id: String,
        node_head: Head,
        provider: P,
        executor: E,
    ) -> (Self, UnboundedSender<ExExEvent>, ExExNotifications<P, E>) {
        let (notification_tx, notification_rx) = mpsc::channel(1);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let notifications =
            ExExNotifications { node_head, provider, executor, notifications: notification_rx };

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
            notifications,
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

/// A stream of [`ExExNotification`]s. The stream will emit notifications for all blocks.
pub struct ExExNotifications<P, E> {
    node_head: Head,
    provider: P,
    executor: E,
    notifications: Receiver<ExExNotification>,
}

impl<P: Debug, E: Debug> Debug for ExExNotifications<P, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExExNotifications")
            .field("provider", &self.provider)
            .field("executor", &self.executor)
            .field("notifications", &self.notifications)
            .finish()
    }
}

impl<P, E> ExExNotifications<P, E> {
    /// Creates a new instance of [`ExExNotifications`].
    pub const fn new(
        node_head: Head,
        provider: P,
        executor: E,
        notifications: Receiver<ExExNotification>,
    ) -> Self {
        Self { node_head, provider, executor, notifications }
    }

    /// Receives the next value for this receiver.
    ///
    /// This method returns `None` if the channel has been closed and there are
    /// no remaining messages in the channel's buffer. This indicates that no
    /// further values can ever be received from this `Receiver`. The channel is
    /// closed when all senders have been dropped, or when [`Receiver::close`] is called.
    ///
    /// # Cancel safety
    ///
    /// This method is cancel safe. If `recv` is used as the event in a
    /// [`tokio::select!`] statement and some other branch
    /// completes first, it is guaranteed that no messages were received on this
    /// channel.
    ///
    /// For full documentation, see [`Receiver::recv`].
    #[deprecated(note = "use `ExExNotifications::next` and its `Stream` implementation instead")]
    pub async fn recv(&mut self) -> Option<ExExNotification> {
        self.notifications.recv().await
    }

    /// Polls to receive the next message on this channel.
    ///
    /// This method returns:
    ///
    ///  * `Poll::Pending` if no messages are available but the channel is not closed, or if a
    ///    spurious failure happens.
    ///  * `Poll::Ready(Some(message))` if a message is available.
    ///  * `Poll::Ready(None)` if the channel has been closed and all messages sent before it was
    ///    closed have been received.
    ///
    /// When the method returns `Poll::Pending`, the `Waker` in the provided
    /// `Context` is scheduled to receive a wakeup when a message is sent on any
    /// receiver, or when the channel is closed.  Note that on multiple calls to
    /// `poll_recv` or `poll_recv_many`, only the `Waker` from the `Context`
    /// passed to the most recent call is scheduled to receive a wakeup.
    ///
    /// If this method returns `Poll::Pending` due to a spurious failure, then
    /// the `Waker` will be notified when the situation causing the spurious
    /// failure has been resolved. Note that receiving such a wakeup does not
    /// guarantee that the next call will succeed — it could fail with another
    /// spurious failure.
    ///
    /// For full documentation, see [`Receiver::poll_recv`].
    #[deprecated(
        note = "use `ExExNotifications::poll_next` and its `Stream` implementation instead"
    )]
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<ExExNotification>> {
        self.notifications.poll_recv(cx)
    }
}

impl<P, E> ExExNotifications<P, E>
where
    P: BlockReader + HeaderProvider + StateProviderFactory + Clone + Unpin + 'static,
    E: BlockExecutorProvider + Clone + Unpin + 'static,
{
    /// Subscribe to notifications with the given head.
    ///
    /// Notifications will be sent starting from the head, not inclusive. For example, if
    /// `head.number == 10`, then the first notification will be with `block.number == 11`.
    pub fn with_head(self, head: ExExHead) -> ExExNotificationsWithHead<P, E> {
        ExExNotificationsWithHead::new(
            self.node_head,
            self.provider,
            self.executor,
            self.notifications,
            head,
        )
    }
}

impl<P: Unpin, E: Unpin> Stream for ExExNotifications<P, E> {
    type Item = ExExNotification;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().notifications.poll_recv(cx)
    }
}

/// A stream of [`ExExNotification`]s. The stream will only emit notifications for blocks that are
/// committed or reverted after the given head.
#[derive(Debug)]
pub struct ExExNotificationsWithHead<P, E> {
    node_head: Head,
    provider: P,
    executor: E,
    notifications: Receiver<ExExNotification>,
    exex_head: ExExHead,
    pending_sync: bool,
    /// The backfill job to run before consuming any notifications.
    backfill_job: Option<StreamBackfillJob<E, P, Chain>>,
    /// Whether we're currently waiting for the node head to catch up to the same height as the
    /// ExEx head.
    node_head_catchup_in_progress: bool,
}

impl<P, E> ExExNotificationsWithHead<P, E>
where
    P: BlockReader + HeaderProvider + StateProviderFactory + Clone + Unpin + 'static,
    E: BlockExecutorProvider + Clone + Unpin + 'static,
{
    /// Creates a new [`ExExNotificationsWithHead`].
    pub const fn new(
        node_head: Head,
        provider: P,
        executor: E,
        notifications: Receiver<ExExNotification>,
        exex_head: ExExHead,
    ) -> Self {
        Self {
            node_head,
            provider,
            executor,
            notifications,
            exex_head,
            pending_sync: true,
            backfill_job: None,
            node_head_catchup_in_progress: false,
        }
    }

    /// Compares the node head against the ExEx head, and synchronizes them in case of a mismatch.
    ///
    /// Possible situations are:
    /// - ExEx is behind the node head (`node_head.number < exex_head.number`).
    ///   - ExEx is on the canonical chain (`exex_head.hash` is found in the node database).
    ///     Backfill from the node database.
    ///   - ExEx is not on the canonical chain (`exex_head.hash` is not found in the node database).
    ///     Unwind the ExEx to the first block matching between the ExEx and the node, and then
    ///     bacfkill from the node database.
    /// - ExEx is at the same block number (`node_head.number == exex_head.number`).
    ///   - ExEx is on the canonical chain (`exex_head.hash` is found in the node database). Nothing
    ///     to do.
    ///   - ExEx is not on the canonical chain (`exex_head.hash` is not found in the node database).
    ///     Unwind the ExEx to the first block matching between the ExEx and the node, and then
    ///     backfill from the node database.
    /// - ExEx is ahead of the node head (`node_head.number > exex_head.number`). Wait until the
    ///   node head catches up to the ExEx head, and then repeat the synchronization process.
    fn synchronize(&mut self) -> eyre::Result<()> {
        debug!(target: "exex::manager", "Synchronizing ExEx head");

        let backfill_job_factory =
            BackfillJobFactory::new(self.executor.clone(), self.provider.clone());
        match self.exex_head.block.number.cmp(&self.node_head.number) {
            std::cmp::Ordering::Less => {
                // ExEx is behind the node head

                if let Some(exex_header) = self.provider.header(&self.exex_head.block.hash)? {
                    // ExEx is on the canonical chain
                    debug!(target: "exex::manager", "ExEx is behind the node head and on the canonical chain");

                    if exex_header.number != self.exex_head.block.number {
                        eyre::bail!("ExEx head number does not match the hash")
                    }

                    // ExEx is on the canonical chain, start backfill
                    let backfill = backfill_job_factory
                        .backfill(self.exex_head.block.number + 1..=self.node_head.number)
                        .into_stream();
                    self.backfill_job = Some(backfill);
                } else {
                    debug!(target: "exex::manager", "ExEx is behind the node head and not on the canonical chain");
                    // ExEx is not on the canonical chain, first unwind it and then backfill

                    // TODO(alexey): unwind and backfill
                    self.backfill_job = None;
                }
            }
            #[allow(clippy::branches_sharing_code)]
            std::cmp::Ordering::Equal => {
                // ExEx is at the same block height as the node head

                if let Some(exex_header) = self.provider.header(&self.exex_head.block.hash)? {
                    // ExEx is on the canonical chain
                    debug!(target: "exex::manager", "ExEx is at the same block height as the node head and on the canonical chain");

                    if exex_header.number != self.exex_head.block.number {
                        eyre::bail!("ExEx head number does not match the hash")
                    }

                    // ExEx is on the canonical chain and the same as the node head, no need to
                    // backfill
                    self.backfill_job = None;
                } else {
                    // ExEx is not on the canonical chain, first unwind it and then backfill
                    debug!(target: "exex::manager", "ExEx is at the same block height as the node head but not on the canonical chain");

                    // TODO(alexey): unwind and backfill
                    self.backfill_job = None;
                }
            }
            std::cmp::Ordering::Greater => {
                debug!(target: "exex::manager", "ExEx is ahead of the node head");

                // ExEx is ahead of the node head

                // TODO(alexey): wait until the node head is at the same height as the ExEx head
                // and then repeat the process above
                self.node_head_catchup_in_progress = true;
            }
        };

        Ok(())
    }
}

impl<P, E> Stream for ExExNotificationsWithHead<P, E>
where
    P: BlockReader + HeaderProvider + StateProviderFactory + Clone + Unpin + 'static,
    E: BlockExecutorProvider + Clone + Unpin + 'static,
{
    type Item = eyre::Result<ExExNotification>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.pending_sync {
            this.synchronize()?;
            this.pending_sync = false;
        }

        if let Some(backfill_job) = &mut this.backfill_job {
            if let Some(chain) = ready!(backfill_job.poll_next_unpin(cx)) {
                return Poll::Ready(Some(Ok(ExExNotification::ChainCommitted {
                    new: Arc::new(chain?),
                })))
            }

            // Backfill job is done, remove it
            this.backfill_job = None;
        }

        loop {
            let Some(notification) = ready!(this.notifications.poll_recv(cx)) else {
                return Poll::Ready(None)
            };

            // 1. Either committed or reverted chain from the notification.
            // 2. Block number of the tip of the canonical chain:
            //   - For committed chain, it's the tip block number.
            //   - For reverted chain, it's the block number preceding the first block in the chain.
            let (chain, tip) = notification
                .committed_chain()
                .map(|chain| (chain.clone(), chain.tip().number))
                .or_else(|| {
                    notification
                        .reverted_chain()
                        .map(|chain| (chain.clone(), chain.first().number - 1))
                })
                .unzip();

            if this.node_head_catchup_in_progress {
                // If we are waiting for the node head to catch up to the same height as the ExEx
                // head, then we need to check if the ExEx is on the canonical chain.

                // Query the chain from the new notification for the ExEx head block number.
                let exex_head_block = chain
                    .as_ref()
                    .and_then(|chain| chain.blocks().get(&this.exex_head.block.number));

                // Compare the hash of the block from the new notification to the ExEx head
                // hash.
                if let Some((block, tip)) = exex_head_block.zip(tip) {
                    if block.hash() == this.exex_head.block.hash {
                        // ExEx is on the canonical chain, proceed with the notification
                        this.node_head_catchup_in_progress = false;
                    } else {
                        // ExEx is not on the canonical chain, synchronize
                        let tip =
                            this.provider.sealed_header(tip)?.ok_or_eyre("node head not found")?;
                        this.node_head = Head::new(
                            tip.number,
                            tip.hash(),
                            tip.difficulty,
                            U256::MAX,
                            tip.timestamp,
                        );
                        this.synchronize()?;
                    }
                }
            }

            if notification
                .committed_chain()
                .or_else(|| notification.reverted_chain())
                .map_or(false, |chain| chain.first().number > this.exex_head.block.number)
            {
                return Poll::Ready(Some(Ok(notification)))
            }
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
    use alloy_primitives::B256;
    use futures::StreamExt;
    use reth_db_common::init::init_genesis;
    use reth_evm_ethereum::execute::EthExecutorProvider;
    use reth_primitives::{Block, BlockNumHash, Header, SealedBlockWithSenders};
    use reth_provider::{
        providers::BlockchainProvider2, test_utils::create_test_provider_factory, BlockReader,
        BlockWriter, Chain,
    };
    use reth_testing_utils::generators::{self, random_block, BlockParams};

    #[tokio::test]
    async fn test_delivers_events() {
        let (mut exex_handle, event_tx, mut _notification_rx) =
            ExExHandle::new("test_exex".to_string(), Head::default(), (), ());

        // Send an event and check that it's delivered correctly
        event_tx.send(ExExEvent::FinishedHeight(42)).unwrap();
        let received_event = exex_handle.receiver.recv().await.unwrap();
        assert_eq!(received_event, ExExEvent::FinishedHeight(42));
    }

    #[tokio::test]
    async fn test_has_exexs() {
        let (exex_handle_1, _, _) =
            ExExHandle::new("test_exex_1".to_string(), Head::default(), (), ());

        assert!(!ExExManager::new(vec![], 0).handle.has_exexs());

        assert!(ExExManager::new(vec![exex_handle_1], 0).handle.has_exexs());
    }

    #[tokio::test]
    async fn test_has_capacity() {
        let (exex_handle_1, _, _) =
            ExExHandle::new("test_exex_1".to_string(), Head::default(), (), ());

        assert!(!ExExManager::new(vec![], 0).handle.has_capacity());

        assert!(ExExManager::new(vec![exex_handle_1], 10).handle.has_capacity());
    }

    #[test]
    fn test_push_notification() {
        let (exex_handle, _, _) = ExExHandle::new("test_exex".to_string(), Head::default(), (), ());

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
        let (exex_handle, _, _) = ExExHandle::new("test_exex".to_string(), Head::default(), (), ());

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
            ExExHandle::new("test_exex".to_string(), Head::default(), (), ());

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
        let (exex_handle1, event_tx1, _) =
            ExExHandle::new("test_exex1".to_string(), Head::default(), (), ());
        let (exex_handle2, event_tx2, _) =
            ExExHandle::new("test_exex2".to_string(), Head::default(), (), ());

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
        let (exex_handle1, event_tx1, _) =
            ExExHandle::new("test_exex1".to_string(), Head::default(), (), ());
        let (exex_handle2, event_tx2, _) =
            ExExHandle::new("test_exex2".to_string(), Head::default(), (), ());

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
        let (exex_handle_1, _, _) =
            ExExHandle::new("test_exex_1".to_string(), Head::default(), (), ());

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
        let (mut exex_handle, _, mut notifications) =
            ExExHandle::new("test_exex".to_string(), Head::default(), (), ());

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
                let received_notification = notifications.next().await.unwrap();
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
        let (mut exex_handle, _, mut notifications) =
            ExExHandle::new("test_exex".to_string(), Head::default(), (), ());

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
                poll_fn(|cx| {
                    // The notification should be skipped, so nothing should be sent.
                    // Check that the receiver channel is indeed empty
                    assert_eq!(
                        notifications.poll_next_unpin(cx),
                        Poll::Pending,
                        "Receiver channel should be empty"
                    );
                    Poll::Ready(())
                })
                .await;
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
        let (mut exex_handle, _, mut notifications) =
            ExExHandle::new("test_exex".to_string(), Head::default(), (), ());

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
                let received_notification = notifications.next().await.unwrap();
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
        let (mut exex_handle, _, mut notifications) =
            ExExHandle::new("test_exex".to_string(), Head::default(), (), ());

        let notification = ExExNotification::ChainReverted { old: Arc::new(Chain::default()) };

        // Even if the finished height is higher than the tip of the new chain, the reorg
        // notification should be received
        exex_handle.finished_height = Some(u64::MAX);

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        // Send the notification
        match exex_handle.send(&mut cx, &(22, notification.clone())) {
            Poll::Ready(Ok(())) => {
                let received_notification = notifications.next().await.unwrap();
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
    async fn exex_notifications_behind_head_canonical() -> eyre::Result<()> {
        let mut rng = generators::rng();

        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory)?;
        let genesis_block = provider_factory
            .block(genesis_hash.into())?
            .ok_or_else(|| eyre::eyre!("genesis block not found"))?;

        let provider = BlockchainProvider2::new(provider_factory.clone())?;

        let node_head_block = random_block(
            &mut rng,
            genesis_block.number + 1,
            BlockParams { parent: Some(genesis_hash), tx_count: Some(0), ..Default::default() },
        );
        let provider_rw = provider_factory.provider_rw()?;
        provider_rw.insert_block(
            node_head_block.clone().seal_with_senders().ok_or_eyre("failed to recover senders")?,
        )?;
        provider_rw.commit()?;

        let node_head = Head {
            number: node_head_block.number,
            hash: node_head_block.hash(),
            ..Default::default()
        };
        let exex_head =
            ExExHead { block: BlockNumHash { number: genesis_block.number, hash: genesis_hash } };

        let notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![random_block(
                    &mut rng,
                    node_head.number + 1,
                    BlockParams { parent: Some(node_head.hash), ..Default::default() },
                )
                .seal_with_senders()
                .ok_or_eyre("failed to recover senders")?],
                Default::default(),
                None,
            )),
        };

        let (notifications_tx, notifications_rx) = mpsc::channel(1);

        notifications_tx.send(notification.clone()).await?;

        let mut notifications = ExExNotifications::new(
            node_head,
            provider,
            EthExecutorProvider::mainnet(),
            notifications_rx,
        )
        .with_head(exex_head);

        // First notification is the backfill of missing blocks from the canonical chain
        assert_eq!(
            notifications.next().await.transpose()?,
            Some(ExExNotification::ChainCommitted {
                new: Arc::new(
                    BackfillJobFactory::new(
                        notifications.executor.clone(),
                        notifications.provider.clone()
                    )
                    .backfill(1..=1)
                    .next()
                    .ok_or_eyre("failed to backfill")??
                )
            })
        );

        // Second notification is the actual notification that we sent before
        assert_eq!(notifications.next().await.transpose()?, Some(notification));

        Ok(())
    }

    #[ignore]
    #[tokio::test]
    async fn exex_notifications_behind_head_non_canonical() -> eyre::Result<()> {
        Ok(())
    }

    #[tokio::test]
    async fn exex_notifications_same_head_canonical() -> eyre::Result<()> {
        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory)?;
        let genesis_block = provider_factory
            .block(genesis_hash.into())?
            .ok_or_else(|| eyre::eyre!("genesis block not found"))?;

        let provider = BlockchainProvider2::new(provider_factory)?;

        let node_head =
            Head { number: genesis_block.number, hash: genesis_hash, ..Default::default() };
        let exex_head =
            ExExHead { block: BlockNumHash { number: node_head.number, hash: node_head.hash } };

        let notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![Block {
                    header: Header {
                        parent_hash: node_head.hash,
                        number: node_head.number + 1,
                        ..Default::default()
                    },
                    ..Default::default()
                }
                .seal_slow()
                .seal_with_senders()
                .ok_or_eyre("failed to recover senders")?],
                Default::default(),
                None,
            )),
        };

        let (notifications_tx, notifications_rx) = mpsc::channel(1);

        notifications_tx.send(notification.clone()).await?;

        let mut notifications = ExExNotifications::new(
            node_head,
            provider,
            EthExecutorProvider::mainnet(),
            notifications_rx,
        )
        .with_head(exex_head);

        let new_notification = notifications.next().await.transpose()?;
        assert_eq!(new_notification, Some(notification));

        Ok(())
    }

    #[ignore]
    #[tokio::test]
    async fn exex_notifications_same_head_non_canonical() -> eyre::Result<()> {
        Ok(())
    }

    #[tokio::test]
    async fn test_notifications_ahead_of_head() -> eyre::Result<()> {
        let mut rng = generators::rng();

        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory)?;
        let genesis_block = provider_factory
            .block(genesis_hash.into())?
            .ok_or_else(|| eyre::eyre!("genesis block not found"))?;

        let provider = BlockchainProvider2::new(provider_factory)?;

        let exex_head_block = random_block(
            &mut rng,
            genesis_block.number + 1,
            BlockParams { parent: Some(genesis_hash), tx_count: Some(0), ..Default::default() },
        );

        let node_head =
            Head { number: genesis_block.number, hash: genesis_hash, ..Default::default() };
        let exex_head = ExExHead {
            block: BlockNumHash { number: exex_head_block.number, hash: exex_head_block.hash() },
        };

        let (notifications_tx, notifications_rx) = mpsc::channel(1);

        notifications_tx
            .send(ExExNotification::ChainCommitted {
                new: Arc::new(Chain::new(
                    vec![exex_head_block
                        .clone()
                        .seal_with_senders()
                        .ok_or_eyre("failed to recover senders")?],
                    Default::default(),
                    None,
                )),
            })
            .await?;

        let mut notifications = ExExNotifications::new(
            node_head,
            provider,
            EthExecutorProvider::mainnet(),
            notifications_rx,
        )
        .with_head(exex_head);

        // First notification is skipped because the node is catching up with the ExEx
        let new_notification = poll_fn(|cx| Poll::Ready(notifications.poll_next_unpin(cx))).await;
        assert!(new_notification.is_pending());

        // Imitate the node catching up with the ExEx by sending a notification for the missing
        // block
        let notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![random_block(
                    &mut rng,
                    exex_head_block.number + 1,
                    BlockParams { parent: Some(exex_head_block.hash()), ..Default::default() },
                )
                .seal_with_senders()
                .ok_or_eyre("failed to recover senders")?],
                Default::default(),
                None,
            )),
        };
        notifications_tx.send(notification.clone()).await?;

        // Second notification is received because the node caught up with the ExEx
        assert_eq!(notifications.next().await.transpose()?, Some(notification));

        Ok(())
    }
}
