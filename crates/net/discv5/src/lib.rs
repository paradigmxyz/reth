use std::{
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

use derive_more::{Deref, DerefMut, From};
use discv5;
use futures::{Stream, StreamExt};
use parking_lot::RwLock;
use pin_project::pin_project;
use reth_discv4::{DiscoveryUpdate, Discv4, HandleDiscV4};
use tokio::sync::{mpsc, watch};
use tokio_stream::{wrappers::ReceiverStream, StreamExt as TokioStreamExt};

pub struct Discv5 {
    discv5: Arc<RwLock<discv5::Discv5>>,
    discv4: Discv4,
    discv5_kbuckets_change_tx: watch::Sender<()>,
}

impl Discv5 {
    pub fn new(
        discv5: Arc<RwLock<discv5::Discv5>>,
        discv4: Discv4,
        discv5_kbuckets_change_tx: watch::Sender<()>,
    ) -> Self {
        Self { discv5, discv4, discv5_kbuckets_change_tx }
    }

    pub fn notify_discv4_of_kbuckets_update(&self) -> Result<(), watch::error::SendError<()>> {
        self.discv5_kbuckets_change_tx.send(())
    }
}

impl HandleDiscV4 for Discv5 {}

#[derive(From)]
pub enum DiscoveryUpdateV5 {
    V5(discv5::Discv5Event),
    V4(DiscoveryUpdate),
}

#[pin_project]
pub struct UpdateStream<S>(#[pin] S);

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

#[pin_project]
pub struct MergedUpdateStream<S>(#[pin] S);

impl<S> Stream for MergedUpdateStream<S>
where
    S: Stream<Item = DiscoveryUpdateV5> + Unpin,
{
    type Item = DiscoveryUpdateV5;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_next_unpin(cx)
    }
}

/// Returns a [`discv5::Event`] stream, that supports downgrading to discv4.
pub fn merge_discovery_streams(
    discv5_event_stream: mpsc::Receiver<discv5::Discv5Event>,
    discv4_update_stream: ReceiverStream<DiscoveryUpdate>,
) -> MergedUpdateStream<impl Stream<Item = DiscoveryUpdateV5>> {
    let discv5_event_stream = UpdateStream(ReceiverStream::new(discv5_event_stream));
    let discv4_update_stream = UpdateStream(discv4_update_stream);

    MergedUpdateStream(discv5_event_stream.merge(discv4_update_stream))
}
