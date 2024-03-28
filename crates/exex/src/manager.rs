use std::{
    collections::VecDeque,
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use reth_primitives::BlockNumber;
use reth_provider::{CanonStateNotification, CanonStateNotifications};
use tokio::sync::{mpsc::UnboundedSender, watch};
use tokio_util::sync::PollSender;

// todo
// - exex height
// - exex channel size
// - manager buffer metrics
// - throughput
struct ExExMetrics;

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
struct ExExManager {
    // todo: exex receiver - get exex events
    /// Channels from the manager to execution extensions.
    exex_tx: Vec<PollSender<CanonStateNotification>>,

    /// [`CanonStateNotification`] channel from the execution stage.
    exec_rx: (), //UnboundedReceiverStream<CanonStateNotification>,
    /// [`CanonStateNotification`] channel from the blockchain tree.
    tree_rx: CanonStateNotifications,

    /// Internal buffer of [`CanonStateNotification`]s.
    buffer: VecDeque<CanonStateNotification>,
    /// Max internal buffer size.
    max_capacity: usize,
    /// Current buffer capacity.
    ///
    /// Used to inform the execution stage of possible batch sizes.
    current_capacity: Arc<AtomicUsize>,

    /// Whether the manager is ready to receive new notifications.
    is_ready: watch::Sender<bool>,

    /// block number for pruner/exec stage (tbd)
    /// todo: this is inclusive, note that in exex too, maybe rename FinishedHeight
    block: watch::Sender<BlockNumber>,

    /// tbd
    handle: ExExManagerHandle,
}

impl Future for ExExManager {
    // todo
    type Output = Result<(), ()>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        todo!()
    }
}

/// TBD
/// note: this is cloneable, should it be
#[derive(Clone)]
pub struct ExExManagerHandle {
    to_exex: UnboundedSender<CanonStateNotification>,
    num_exexs: usize,
    is_ready: watch::Receiver<bool>,
    current_capacity: Arc<AtomicUsize>,
    block: watch::Receiver<BlockNumber>,
}

impl ExExManagerHandle {
    pub fn should_send(&mut self, block_number: BlockNumber) -> bool {
        let has_exexs = self.num_exexs > 0;
        let within_threshold = block_number >= *self.block.borrow_and_update();

        has_exexs && within_threshold && self.has_capacity()
    }

    pub fn has_capacity(&self) -> bool {
        self.current_capacity.load(Ordering::Relaxed) > 0
    }

    pub async fn ready(&self) {
        todo!()
    }
}
