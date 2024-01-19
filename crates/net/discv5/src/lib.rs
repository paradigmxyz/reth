use std::{
    pin::Pin,
    task::{Context, Poll},
};

use derive_more::{Deref, DerefMut, From};
use discv5::{self, Discv5};
use futures::{Stream, StreamExt};
use reth_discv4::{DiscoveryUpdate, Discv4};
use tokio::sync::{mpsc, watch};
use tokio_stream::{wrappers::ReceiverStream, StreamExt as TokioStreamExt};

struct DiscoveryV5 {
    discv5_handle: Discv5,
    discv4_handle: Discv4,
}

#[derive(From)]
enum DiscoveryUpdateV5 {
    V5(discv5::Event),
    V4(DiscoveryUpdate),
}

#[derive(Deref, DerefMut)]
struct UpdateStream<S>(S);

impl<S, I> Stream for UpdateStream<S>
where
    S: Stream<Item = I>,
    DiscoveryUpdateV5: From<I>,
{
    type Item = DiscoveryUpdateV5;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.poll_next_unpin(cx)
    }
}

/// Returns a [`discv5::Event`] stream, that supports downgrading to discv4.
pub fn merge_versions(
    discv5_event_stream: mpsc::Receiver<discv5::Event>,
    discv4_update_stream: ReceiverStream<DiscoveryUpdate>,
) -> impl Stream<Item = DiscoveryUpdateV5> {
    let discv5_event_stream = UpdateStream(ReceiverStream::new(discv5_event_stream));
    let discv4_update_stream = UpdateStream(discv4_update_stream);

    discv5_event_stream.merge(discv4_update_stream)
}
