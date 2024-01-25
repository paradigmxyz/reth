//! Wrapper for [`discv5::Discv5`] that supports downgrade to [`Discv4`].

use std::{
    fmt,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

use derive_more::From;
use futures::{
    stream::{select, Select},
    Stream, StreamExt,
};
use parking_lot::RwLock;
use reth_discv4::{DiscoveryUpdate, Discv4, HandleDiscovery};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;

/// Reth Discv5 type, wraps [`discv5::Discv5`] supporting downgrade to [`Discv4`].
pub struct Discv5 {
    _discv5: Arc<RwLock<discv5::Discv5>>,
    discv4: Discv4,
    discv5_kbuckets_change_tx: watch::Sender<()>,
}

impl Discv5 {
    /// Returns a new [`Discv5`] handle.
    pub fn new(
        discv5: Arc<RwLock<discv5::Discv5>>,
        discv4: Discv4,
        discv5_kbuckets_change_tx: watch::Sender<()>,
    ) -> Self {
        Self { _discv5: discv5, discv4, discv5_kbuckets_change_tx }
    }

    /// Notifies [`Discv4`] that [discv5::Discv5]'s kbucktes have been updated. This brings
    /// [`Discv4`] to update its mirror of the [discv5::Discv5] kbucktes upon next
    /// [`reth_discv4::Neighbours`] message.
    pub fn notify_discv4_of_kbuckets_update(&self) -> Result<(), watch::error::SendError<()>> {
        self.discv5_kbuckets_change_tx.send(())
    }
}

impl HandleDiscovery for Discv5 {
    // todo: delegate methods to both discv5 and discv4
}

impl fmt::Debug for Discv5 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("Discv5");

        debug_struct.field("discv5", &"{ .. }");
        debug_struct.field("discv4", &self.discv4);
        debug_struct.field("discv5_kbuckets_change_tx", &self.discv5_kbuckets_change_tx);

        debug_struct.finish()
    }
}

/// Wrapper around update type used in [`discv5::Discv5`] and [`Discv4`].
#[derive(Debug, From)]
pub enum DiscoveryUpdateV5 {
    /// A [`discv5::Discv5`] update.
    V5(discv5::Discv5Event),
    /// A [`Discv4`] update.
    V4(DiscoveryUpdate),
}

/// Stream wrapper for streams producing types that can convert to [`DiscoveryUpdateV5`].
#[derive(Debug)]
pub struct UpdateStream<S>(S);

impl<S, I> Stream for UpdateStream<S>
where
    S: Stream<Item = I> + Unpin,
    DiscoveryUpdateV5: From<I>,
{
    type Item = DiscoveryUpdateV5;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(ready!(self.0.poll_next_unpin(cx)).map(DiscoveryUpdateV5::from))
    }
}

/// A stream that polls update streams from [`discv5::Discv5`] and [`Discv4`] in round-robin
/// fashion.
pub type MergedUpdateStream = Select<
    UpdateStream<ReceiverStream<discv5::Discv5Event>>,
    UpdateStream<ReceiverStream<DiscoveryUpdate>>,
>;

/// Returns a merged stream of [`discv5::Discv5Event`]s and [`DiscoveryUpdate`]s, that supports
/// downgrading to discv4.
pub fn merge_discovery_streams(
    discv5_event_stream: mpsc::Receiver<discv5::Discv5Event>,
    discv4_update_stream: ReceiverStream<DiscoveryUpdate>,
) -> Select<
    UpdateStream<ReceiverStream<discv5::Discv5Event>>,
    UpdateStream<ReceiverStream<DiscoveryUpdate>>,
> {
    let discv5_event_stream = UpdateStream(ReceiverStream::new(discv5_event_stream));
    let discv4_update_stream = UpdateStream(discv4_update_stream);

    select(discv5_event_stream, discv4_update_stream)
}
