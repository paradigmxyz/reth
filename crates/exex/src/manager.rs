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
use reth_primitives::BlockNumber;
use reth_provider::{CanonStateNotification, CanonStateNotifications};
use tokio::sync::{
    mpsc::{self, Receiver, Sender, UnboundedReceiver, UnboundedSender},
    watch,
};
use tokio_util::sync::{PollSendError, PollSender};

// todo: we need to figure out how to keep track of some of these things per exex
// currently, the only path forward seems to be adding an exex id to every event, since we only
// have one receiver for ExExEvents
//
// things to track:
// - exex height
// - exex channel size
// - manager buffer metrics
// - throughput
#[derive(Debug)]
struct ExExMetrics;

#[derive(Debug)]
pub struct ExExHandle {
    /// The execution extension's ID.
    id: String,
    /// Channel to send [`CanonStateNotification`]s to the ExEx.
    sender: PollSender<CanonStateNotification>,
    /// Channel to receive [`ExExEvent`]s from the ExEx.
    receiver: Receiver<ExExEvent>,
    /// The ID of the next notification to send to this ExEx.
    next_notification_id: usize,
}

impl ExExHandle {
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
                Poll::Ready(Ok(()))
            }
            Err(err) => Poll::Ready(Err(err)),
        }
    }
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
    /// [`CanonStateNotification`] channel from the blockchain tree.
    tree_rx: CanonStateNotifications,

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
}

impl ExExManager {
    /// Create a new [`ExExManager`].
    ///
    /// You must provide:
    /// - An [`ExExHandle`] for each ExEx
    /// - The receiving side of [`CanonStateNotification`]s from the blockchain tree
    /// - The maximum capacity of the notification buffer in the manager
    pub fn new(
        handles: Vec<ExExHandle>,
        tree_rx: CanonStateNotifications,
        max_capacity: usize,
    ) -> Self {
        let num_exexs = handles.len();

        let (handle_tx, handle_rx) = mpsc::unbounded_channel();
        let (is_ready_tx, is_ready_rx) = watch::channel(true);
        let (block_tx, block_rx) = watch::channel(0);

        let current_capacity = Arc::new(AtomicUsize::new(max_capacity));

        Self {
            exex_handles: handles,

            handle_rx,
            tree_rx,

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
        }

        // remove processed buffered events
        self.buffer.retain(|&(id, _)| id < min_id);
        self.min_id = min_id;

        // update capacity
        self.update_capacity();

        // todo: handle incoming exex events
        // this requires keeping track of exex id -> finished block number, and updating the block
        // watch channel iff all exex's have sent this event at least once

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

    /// Whether there is capacity in the ExEx manager's internal notification buffer.
    ///
    /// If this returns `false`, the owner of the handle should **NOT** send new notifications over
    /// the channel until the manager is ready again, as this can lead to unbounded memory growth.
    pub fn has_capacity(&self) -> bool {
        self.current_capacity.load(Ordering::Relaxed) > 0
    }

    pub async fn ready(&self) {
        todo!()
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
