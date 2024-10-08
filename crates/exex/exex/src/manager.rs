use crate::{
    wal::Wal, ExExEvent, ExExNotification, ExExNotifications, FinishedExExHeight, WalHandle,
};
use futures::StreamExt;
use itertools::Itertools;
use metrics::Gauge;
use reth_chain_state::ForkChoiceStream;
use reth_chainspec::Head;
use reth_metrics::{metrics::Counter, Metrics};
use reth_primitives::{BlockNumHash, SealedHeader};
use reth_provider::HeaderProvider;
use reth_tracing::tracing::debug;
use std::{
    collections::VecDeque,
    fmt::Debug,
    future::{poll_fn, Future},
    ops::Not,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
};
use tokio::sync::{
    mpsc::{self, error::SendError, UnboundedReceiver, UnboundedSender},
    watch,
};
use tokio_util::sync::{PollSendError, PollSender, ReusableBoxFuture};

/// Default max size of the internal state notifications buffer.
///
/// 1024 notifications in the buffer is 3.5 hours of mainnet blocks,
/// or 17 minutes of 1-second blocks.
pub const DEFAULT_EXEX_MANAGER_CAPACITY: usize = 1024;

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
    /// The finished block of the `ExEx`.
    ///
    /// If this is `None`, the `ExEx` has not emitted a `FinishedHeight` event.
    finished_height: Option<BlockNumHash>,
}

impl ExExHandle {
    /// Create a new handle for the given `ExEx`.
    ///
    /// Returns the handle, as well as a [`UnboundedSender`] for [`ExExEvent`]s and a
    /// [`mpsc::Receiver`] for [`ExExNotification`]s that should be given to the `ExEx`.
    pub fn new<P, E>(
        id: String,
        node_head: Head,
        provider: P,
        executor: E,
        wal_handle: WalHandle,
    ) -> (Self, UnboundedSender<ExExEvent>, ExExNotifications<P, E>) {
        let (notification_tx, notification_rx) = mpsc::channel(1);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let notifications =
            ExExNotifications::new(node_head, provider, executor, notification_rx, wal_handle);

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
                    if finished_height.number >= new.tip().number {
                        debug!(
                            target: "exex::manager",
                            exex_id = %self.id,
                            %notification_id,
                            ?finished_height,
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
            target: "exex::manager",
            exex_id = %self.id,
            %notification_id,
            "Reserving slot for notification"
        );
        match self.sender.poll_reserve(cx) {
            Poll::Ready(Ok(())) => (),
            other => return other,
        }

        debug!(
            target: "exex::manager",
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
#[metrics(scope = "exex.manager")]
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
pub struct ExExManager<P> {
    /// Provider for querying headers.
    provider: P,

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

    /// Write-Ahead Log for the [`ExExNotification`]s.
    wal: Wal,
    /// A stream of finalized headers.
    finalized_header_stream: ForkChoiceStream<SealedHeader>,

    /// A handle to the `ExEx` manager.
    handle: ExExManagerHandle,
    /// Metrics for the `ExEx` manager.
    metrics: ExExManagerMetrics,
}

impl<P> ExExManager<P> {
    /// Create a new [`ExExManager`].
    ///
    /// You must provide an [`ExExHandle`] for each `ExEx` and the maximum capacity of the
    /// notification buffer in the manager.
    ///
    /// When the capacity is exceeded (which can happen if an `ExEx` is slow) no one can send
    /// notifications over [`ExExManagerHandle`]s until there is capacity again.
    pub fn new(
        provider: P,
        handles: Vec<ExExHandle>,
        max_capacity: usize,
        wal: Wal,
        finalized_header_stream: ForkChoiceStream<SealedHeader>,
    ) -> Self {
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
            provider,

            exex_handles: handles,

            handle_rx,

            min_id: 0,
            next_id: 0,
            buffer: VecDeque::with_capacity(max_capacity),
            max_capacity,
            current_capacity: Arc::clone(&current_capacity),

            is_ready: is_ready_tx,
            finished_height: finished_height_tx,

            wal,
            finalized_header_stream,

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

impl<P> ExExManager<P>
where
    P: HeaderProvider,
{
    /// Finalizes the WAL according to the passed finalized header.
    ///
    /// This function checks if all ExExes are on the canonical chain and finalizes the WAL if
    /// necessary.
    fn finalize_wal(&self, finalized_header: SealedHeader) -> eyre::Result<()> {
        debug!(target: "exex::manager", header = ?finalized_header.num_hash(), "Received finalized header");

        // Check if all ExExes are on the canonical chain
        let exex_finished_heights = self
            .exex_handles
            .iter()
            // Get ID and finished height for each ExEx
            .map(|exex_handle| (&exex_handle.id, exex_handle.finished_height))
            // Deduplicate all hashes
            .unique_by(|(_, num_hash)| num_hash.map(|num_hash| num_hash.hash))
            // Check if hashes are canonical
            .map(|(exex_id, num_hash)| {
                num_hash.map_or(Ok((exex_id, num_hash, false)), |num_hash| {
                    self.provider
                        .is_known(&num_hash.hash)
                        // Save the ExEx ID, finished height, and whether the hash is canonical
                        .map(|is_canonical| (exex_id, Some(num_hash), is_canonical))
                })
            })
            // We collect here to be able to log the unfinalized ExExes below
            .collect::<Result<Vec<_>, _>>()?;
        if exex_finished_heights.iter().all(|(_, _, is_canonical)| *is_canonical) {
            // If there is a finalized header and all ExExs are on the canonical chain, finalize
            // the WAL with either the lowest finished height among all ExExes, or finalized header
            // – whichever is lower.
            let lowest_finished_height = exex_finished_heights
                .iter()
                .copied()
                .filter_map(|(_, num_hash, _)| num_hash)
                .chain([(finalized_header.num_hash())])
                .min_by_key(|num_hash| num_hash.number)
                .unwrap();

            self.wal.finalize(lowest_finished_height)?;
        } else {
            let unfinalized_exexes = exex_finished_heights
                .into_iter()
                .filter_map(|(exex_id, num_hash, is_canonical)| {
                    is_canonical.not().then_some((exex_id, num_hash))
                })
                .format_with(", ", |(exex_id, num_hash), f| {
                    f(&format_args!("{exex_id} = {num_hash:?}"))
                })
                // We need this because `debug!` uses the argument twice when formatting the final
                // log message, but the result of `format_with` can only be used once
                .to_string();
            debug!(
                target: "exex::manager",
                %unfinalized_exexes,
                "Not all ExExes are on the canonical chain, can't finalize the WAL"
            );
        }

        Ok(())
    }
}

impl<P> Future for ExExManager<P>
where
    P: HeaderProvider + Unpin + 'static,
{
    type Output = eyre::Result<()>;

    /// Main loop of the [`ExExManager`]. The order of operations is as follows:
    /// 1. Handle incoming ExEx events. We do it before finalizing the WAL, because it depends on
    ///    the latest state of [`ExExEvent::FinishedHeight`] events.
    /// 2. Finalize the WAL with the finalized header, if necessary.
    /// 3. Drain [`ExExManagerHandle`] notifications, push them to the internal buffer and update
    ///    the internal buffer capacity.
    /// 5. Send notifications from the internal buffer to those ExExes that are ready to receive new
    ///    notifications.
    /// 5. Remove notifications from the internal buffer that have been sent to **all** ExExes and
    ///    update the internal buffer capacity.
    /// 6. Update the channel with the lowest [`FinishedExExHeight`] among all ExExes.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Handle incoming ExEx events
        for exex in &mut this.exex_handles {
            while let Poll::Ready(Some(event)) = exex.receiver.poll_recv(cx) {
                debug!(target: "exex::manager", exex_id = %exex.id, ?event, "Received event from ExEx");
                exex.metrics.events_sent_total.increment(1);
                match event {
                    ExExEvent::FinishedHeight(height) => exex.finished_height = Some(height),
                }
            }
        }

        // Drain the finalized header stream and finalize the WAL with the last header
        let mut last_finalized_header = None;
        while let Poll::Ready(finalized_header) = this.finalized_header_stream.poll_next_unpin(cx) {
            last_finalized_header = finalized_header;
        }
        if let Some(header) = last_finalized_header {
            this.finalize_wal(header)?;
        }

        // Drain handle notifications
        while this.buffer.len() < this.max_capacity {
            if let Poll::Ready(Some(notification)) = this.handle_rx.poll_recv(cx) {
                debug!(
                    target: "exex::manager",
                    committed_tip = ?notification.committed_chain().map(|chain| chain.tip().number),
                    reverted_tip = ?notification.reverted_chain().map(|chain| chain.tip().number),
                    "Received new notification"
                );
                this.wal.commit(&notification)?;
                this.push_notification(notification);
                continue
            }
            break
        }

        // Update capacity
        this.update_capacity();

        // Advance all poll senders
        let mut min_id = usize::MAX;
        for idx in (0..this.exex_handles.len()).rev() {
            let mut exex = this.exex_handles.swap_remove(idx);

            // It is a logic error for this to ever underflow since the manager manages the
            // notification IDs
            let notification_index = exex
                .next_notification_id
                .checked_sub(this.min_id)
                .expect("exex expected notification ID outside the manager's range");
            if let Some(notification) = this.buffer.get(notification_index) {
                if let Poll::Ready(Err(err)) = exex.send(cx, notification) {
                    // The channel was closed, which is irrecoverable for the manager
                    return Poll::Ready(Err(err.into()))
                }
            }
            min_id = min_id.min(exex.next_notification_id);
            this.exex_handles.push(exex);
        }

        // Remove processed buffered notifications
        debug!(target: "exex::manager", %min_id, "Updating lowest notification id in buffer");
        this.buffer.retain(|&(id, _)| id >= min_id);
        this.min_id = min_id;

        // Update capacity
        this.update_capacity();

        // Update watch channel block number
        let finished_height = this.exex_handles.iter_mut().try_fold(u64::MAX, |curr, exex| {
            exex.finished_height.map_or(Err(()), |height| Ok(height.number.min(curr)))
        });
        if let Ok(finished_height) = finished_height {
            let _ = this.finished_height.send(FinishedExExHeight::Height(finished_height));
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
    use rand::Rng;
    use reth_db_common::init::init_genesis;
    use reth_evm_ethereum::execute::EthExecutorProvider;
    use reth_primitives::SealedBlockWithSenders;
    use reth_provider::{
        providers::BlockchainProvider2, test_utils::create_test_provider_factory, BlockReader,
        Chain, TransactionVariant,
    };
    use reth_testing_utils::generators;

    fn empty_finalized_header_stream() -> ForkChoiceStream<SealedHeader> {
        let (tx, rx) = watch::channel(None);
        // Do not drop the sender, otherwise the receiver will always return an error
        std::mem::forget(tx);
        ForkChoiceStream::new(rx)
    }

    #[tokio::test]
    async fn test_delivers_events() {
        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let (mut exex_handle, event_tx, mut _notification_rx) =
            ExExHandle::new("test_exex".to_string(), Head::default(), (), (), wal.handle());

        // Send an event and check that it's delivered correctly
        let event = ExExEvent::FinishedHeight(BlockNumHash::new(42, B256::random()));
        event_tx.send(event).unwrap();
        let received_event = exex_handle.receiver.recv().await.unwrap();
        assert_eq!(received_event, event);
    }

    #[tokio::test]
    async fn test_has_exexs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let (exex_handle_1, _, _) =
            ExExHandle::new("test_exex_1".to_string(), Head::default(), (), (), wal.handle());

        assert!(!ExExManager::new((), vec![], 0, wal.clone(), empty_finalized_header_stream())
            .handle
            .has_exexs());

        assert!(ExExManager::new((), vec![exex_handle_1], 0, wal, empty_finalized_header_stream())
            .handle
            .has_exexs());
    }

    #[tokio::test]
    async fn test_has_capacity() {
        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let (exex_handle_1, _, _) =
            ExExHandle::new("test_exex_1".to_string(), Head::default(), (), (), wal.handle());

        assert!(!ExExManager::new((), vec![], 0, wal.clone(), empty_finalized_header_stream())
            .handle
            .has_capacity());

        assert!(ExExManager::new(
            (),
            vec![exex_handle_1],
            10,
            wal,
            empty_finalized_header_stream()
        )
        .handle
        .has_capacity());
    }

    #[test]
    fn test_push_notification() {
        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let (exex_handle, _, _) =
            ExExHandle::new("test_exex".to_string(), Head::default(), (), (), wal.handle());

        // Create a mock ExExManager and add the exex_handle to it
        let mut exex_manager =
            ExExManager::new((), vec![exex_handle], 10, wal, empty_finalized_header_stream());

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
        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let (exex_handle, _, _) =
            ExExHandle::new("test_exex".to_string(), Head::default(), (), (), wal.handle());

        // Create a mock ExExManager and add the exex_handle to it
        let max_capacity = 5;
        let mut exex_manager = ExExManager::new(
            (),
            vec![exex_handle],
            max_capacity,
            wal,
            empty_finalized_header_stream(),
        );

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
        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let provider_factory = create_test_provider_factory();

        let (exex_handle, event_tx, mut _notification_rx) =
            ExExHandle::new("test_exex".to_string(), Head::default(), (), (), wal.handle());

        // Check initial block height
        assert!(exex_handle.finished_height.is_none());

        // Update the block height via an event
        let block = BlockNumHash::new(42, B256::random());
        event_tx.send(ExExEvent::FinishedHeight(block)).unwrap();

        // Create a mock ExExManager and add the exex_handle to it
        let exex_manager = ExExManager::new(
            provider_factory,
            vec![exex_handle],
            10,
            Wal::new(temp_dir.path()).unwrap(),
            empty_finalized_header_stream(),
        );

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        // Pin the ExExManager to call the poll method
        let mut pinned_manager = std::pin::pin!(exex_manager);
        let _ = pinned_manager.as_mut().poll(&mut cx);

        // Check that the block height was updated
        let updated_exex_handle = &pinned_manager.exex_handles[0];
        assert_eq!(updated_exex_handle.finished_height, Some(block));

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
        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let provider_factory = create_test_provider_factory();

        // Create two `ExExHandle` instances
        let (exex_handle1, event_tx1, _) =
            ExExHandle::new("test_exex1".to_string(), Head::default(), (), (), wal.handle());
        let (exex_handle2, event_tx2, _) =
            ExExHandle::new("test_exex2".to_string(), Head::default(), (), (), wal.handle());

        let block1 = BlockNumHash::new(42, B256::random());
        let block2 = BlockNumHash::new(10, B256::random());

        // Send events to update the block heights of the two handles, with the second being lower
        event_tx1.send(ExExEvent::FinishedHeight(block1)).unwrap();
        event_tx2.send(ExExEvent::FinishedHeight(block2)).unwrap();

        let exex_manager = ExExManager::new(
            provider_factory,
            vec![exex_handle1, exex_handle2],
            10,
            Wal::new(temp_dir.path()).unwrap(),
            empty_finalized_header_stream(),
        );

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
        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let provider_factory = create_test_provider_factory();

        // Create two `ExExHandle` instances
        let (exex_handle1, event_tx1, _) =
            ExExHandle::new("test_exex1".to_string(), Head::default(), (), (), wal.handle());
        let (exex_handle2, event_tx2, _) =
            ExExHandle::new("test_exex2".to_string(), Head::default(), (), (), wal.handle());

        // Assert that the initial block height is `None` for the first `ExExHandle`.
        assert!(exex_handle1.finished_height.is_none());

        let block1 = BlockNumHash::new(42, B256::random());
        let block2 = BlockNumHash::new(100, B256::random());

        // Send events to update the block heights of the two handles, with the second being higher.
        event_tx1.send(ExExEvent::FinishedHeight(block1)).unwrap();
        event_tx2.send(ExExEvent::FinishedHeight(block2)).unwrap();

        let exex_manager = ExExManager::new(
            provider_factory,
            vec![exex_handle1, exex_handle2],
            10,
            Wal::new(temp_dir.path()).unwrap(),
            empty_finalized_header_stream(),
        );

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
        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let provider_factory = create_test_provider_factory();

        let (exex_handle_1, _, _) =
            ExExHandle::new("test_exex_1".to_string(), Head::default(), (), (), wal.handle());

        // Create an ExExManager with a small max capacity
        let max_capacity = 2;
        let mut exex_manager = ExExManager::new(
            provider_factory,
            vec![exex_handle_1],
            max_capacity,
            Wal::new(temp_dir.path()).unwrap(),
            empty_finalized_header_stream(),
        );

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
        let provider_factory = create_test_provider_factory();
        init_genesis(&provider_factory).unwrap();
        let provider = BlockchainProvider2::new(provider_factory).unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let (mut exex_handle, _, mut notifications) = ExExHandle::new(
            "test_exex".to_string(),
            Head::default(),
            provider,
            EthExecutorProvider::mainnet(),
            wal.handle(),
        );

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
                let received_notification = notifications.next().await.unwrap().unwrap();
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
        let provider_factory = create_test_provider_factory();
        init_genesis(&provider_factory).unwrap();
        let provider = BlockchainProvider2::new(provider_factory).unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let (mut exex_handle, _, mut notifications) = ExExHandle::new(
            "test_exex".to_string(),
            Head::default(),
            provider,
            EthExecutorProvider::mainnet(),
            wal.handle(),
        );

        // Set finished_height to a value higher than the block tip
        exex_handle.finished_height = Some(BlockNumHash::new(15, B256::random()));

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
                    assert!(notifications.poll_next_unpin(cx).is_pending());
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
        let provider_factory = create_test_provider_factory();
        init_genesis(&provider_factory).unwrap();
        let provider = BlockchainProvider2::new(provider_factory).unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let (mut exex_handle, _, mut notifications) = ExExHandle::new(
            "test_exex".to_string(),
            Head::default(),
            provider,
            EthExecutorProvider::mainnet(),
            wal.handle(),
        );

        let notification = ExExNotification::ChainReorged {
            old: Arc::new(Chain::default()),
            new: Arc::new(Chain::default()),
        };

        // Even if the finished height is higher than the tip of the new chain, the reorg
        // notification should be received
        exex_handle.finished_height = Some(BlockNumHash::new(u64::MAX, B256::random()));

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        // Send the notification
        match exex_handle.send(&mut cx, &(22, notification.clone())) {
            Poll::Ready(Ok(())) => {
                let received_notification = notifications.next().await.unwrap().unwrap();
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
        let provider_factory = create_test_provider_factory();
        init_genesis(&provider_factory).unwrap();
        let provider = BlockchainProvider2::new(provider_factory).unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let (mut exex_handle, _, mut notifications) = ExExHandle::new(
            "test_exex".to_string(),
            Head::default(),
            provider,
            EthExecutorProvider::mainnet(),
            wal.handle(),
        );

        let notification = ExExNotification::ChainReverted { old: Arc::new(Chain::default()) };

        // Even if the finished height is higher than the tip of the new chain, the reorg
        // notification should be received
        exex_handle.finished_height = Some(BlockNumHash::new(u64::MAX, B256::random()));

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        // Send the notification
        match exex_handle.send(&mut cx, &(22, notification.clone())) {
            Poll::Ready(Ok(())) => {
                let received_notification = notifications.next().await.unwrap().unwrap();
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
    async fn test_exex_wal() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        let mut rng = generators::rng();

        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory).unwrap();
        let genesis_block = provider_factory
            .sealed_block_with_senders(genesis_hash.into(), TransactionVariant::NoHash)
            .unwrap()
            .ok_or_else(|| eyre::eyre!("genesis block not found"))?;
        let provider = BlockchainProvider2::new(provider_factory).unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let (exex_handle, events_tx, mut notifications) = ExExHandle::new(
            "test_exex".to_string(),
            Head::default(),
            provider.clone(),
            EthExecutorProvider::mainnet(),
            wal.handle(),
        );

        let notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(vec![genesis_block.clone()], Default::default(), None)),
        };

        let (finalized_headers_tx, rx) = watch::channel(None);
        let finalized_header_stream = ForkChoiceStream::new(rx);

        let mut exex_manager = std::pin::pin!(ExExManager::new(
            provider,
            vec![exex_handle],
            1,
            wal,
            finalized_header_stream
        ));

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        exex_manager.handle().send(notification.clone())?;

        assert!(exex_manager.as_mut().poll(&mut cx)?.is_pending());
        assert_eq!(notifications.next().await.unwrap().unwrap(), notification.clone());
        assert_eq!(
            exex_manager.wal.iter_notifications()?.collect::<eyre::Result<Vec<_>>>()?,
            [notification.clone()]
        );

        finalized_headers_tx.send(Some(genesis_block.header.clone()))?;
        assert!(exex_manager.as_mut().poll(&mut cx).is_pending());
        // WAL isn't finalized because the ExEx didn't emit the `FinishedHeight` event
        assert_eq!(
            exex_manager.wal.iter_notifications()?.collect::<eyre::Result<Vec<_>>>()?,
            [notification.clone()]
        );

        // Send a `FinishedHeight` event with a non-canonical block
        events_tx
            .send(ExExEvent::FinishedHeight((rng.gen::<u64>(), rng.gen::<B256>()).into()))
            .unwrap();

        finalized_headers_tx.send(Some(genesis_block.header.clone()))?;
        assert!(exex_manager.as_mut().poll(&mut cx).is_pending());
        // WAL isn't finalized because the ExEx emitted a `FinishedHeight` event with a
        // non-canonical block
        assert_eq!(
            exex_manager.wal.iter_notifications()?.collect::<eyre::Result<Vec<_>>>()?,
            [notification]
        );

        // Send a `FinishedHeight` event with a canonical block
        events_tx.send(ExExEvent::FinishedHeight(genesis_block.num_hash())).unwrap();

        finalized_headers_tx.send(Some(genesis_block.header.clone()))?;
        assert!(exex_manager.as_mut().poll(&mut cx).is_pending());
        // WAL is finalized
        assert!(exex_manager.wal.iter_notifications()?.next().is_none());

        Ok(())
    }
}
