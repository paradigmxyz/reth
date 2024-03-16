//! Wrapper around [`discv5::Discv5`] that supports downgrade to [`Discv4`].

use std::{
    net::IpAddr,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

use derive_more::From;
use futures::{
    stream::{select, Select},
    Stream, StreamExt,
};
use reth_discv4::{DiscoveryUpdate, Discv4};
use reth_net_common::discovery::{HandleDiscovery, NodeFromExternalSource};
use reth_primitives::{
    bytes::{Bytes, BytesMut},
    PeerId,
};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;

use crate::DiscV5;

/// Errors interfacing between [`discv5::Discv5`] and [`Discv4`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failure sending notification to [`Discv4`].
    #[error("failed notifying discv4 of update to discv5 kbuckets")]
    NotifyKBucketsUpdateFailed(watch::error::SendError<()>),
    /// Error interfacing with [`discv5::Discv5`].
    #[error(transparent)]
    DiscV5Error(super::Error),
}

/// Wraps [`discv5::Discv5`] supporting downgrade to [`Discv4`].
#[derive(Debug)]
pub struct DiscV5WithV4Downgrade {
    discv5: Arc<DiscV5>,
    discv4: Discv4,
}

impl DiscV5WithV4Downgrade {
    /// Returns a new combined handle.
    pub fn new(discv5: Arc<DiscV5>, discv4: Discv4) -> Self {
        Self { discv5, discv4 }
    }

    /// Exposes methods on [`Discv4`] that take a reference to self.
    pub fn with_discv4<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Discv4) -> R,
    {
        f(&self.discv4)
    }

    /// Exposes methods on [`Discv4`] that take a mutable reference to self.
    pub fn with_discv4_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Discv4) -> R,
    {
        f(&mut self.discv4)
    }

    /// Exposes API of [`discv5::Discv5`].
    pub fn with_discv5<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&DiscV5) -> R,
    {
        f(&self.discv5)
    }
}

impl HandleDiscovery for DiscV5WithV4Downgrade {
    fn add_node_to_routing_table(
        &self,
        node_record: NodeFromExternalSource,
    ) -> Result<(), impl std::error::Error> {
        self.discv5.add_node(node_record).map_err(Error::DiscV5Error)
    }

    fn set_eip868_in_local_enr(&self, key: Vec<u8>, rlp: Bytes) {
        self.discv5.set_eip868_in_local_enr(key.clone(), rlp.clone());
        self.discv4.set_eip868_in_local_enr(key, rlp)
    }

    fn encode_and_set_eip868_in_local_enr(&self, key: Vec<u8>, value: impl alloy_rlp::Encodable) {
        let mut buf = BytesMut::new();
        value.encode(&mut buf);
        self.set_eip868_in_local_enr(key, buf.freeze())
    }

    fn ban_peer_by_ip_and_node_id(&self, peer_id: PeerId, ip: IpAddr) {
        self.discv5.ban_peer_by_ip_and_node_id(peer_id, ip);
        self.discv4.ban_peer_by_ip_and_node_id(peer_id, ip)
    }

    fn ban_peer_by_ip(&self, ip: IpAddr) {
        self.discv5.ban_peer_by_ip(ip);
        self.discv4.ban_peer_by_ip(ip)
    }
}

/// Wrapper around update type used in [`discv5::Discv5`] and [`Discv4`].
#[derive(Debug, From)]
pub enum DiscoveryUpdateV5 {
    /// A [`discv5::Event`], an update from [`discv5::Discv5`].
    V5(discv5::Event),
    /// A [`DiscoveryUpdate`], an update from [`Discv4`].
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
#[derive(Debug)]
pub struct MergedUpdateStream {
    inner: Select<
        UpdateStream<ReceiverStream<discv5::Event>>,
        UpdateStream<ReceiverStream<DiscoveryUpdate>>,
    >,
    discv5_kbuckets_change_tx: watch::Sender<()>,
}

impl MergedUpdateStream {
    /// Returns a merged stream of [`discv5::Event`]s and [`DiscoveryUpdate`]s, that supports
    /// downgrading to discv4.
    pub fn merge_discovery_streams(
        discv5_event_stream: mpsc::Receiver<discv5::Event>,
        discv4_update_stream: ReceiverStream<DiscoveryUpdate>,
        discv5_kbuckets_change_tx: watch::Sender<()>,
    ) -> Self {
        let discv5_event_stream = UpdateStream(ReceiverStream::new(discv5_event_stream));
        let discv4_update_stream = UpdateStream(discv4_update_stream);

        Self { inner: select(discv5_event_stream, discv4_update_stream), discv5_kbuckets_change_tx }
    }

    /// Notifies [`Discv4`] that [discv5::Discv5]'s kbuckets have been updated and that [`Discv4`]
    /// should update its mirror thereof.
    fn notify_discv4_of_kbuckets_update(&self) -> Result<(), watch::error::SendError<()>> {
        self.discv5_kbuckets_change_tx.send(())
    }
}

impl Stream for MergedUpdateStream {
    type Item = DiscoveryUpdateV5; // todo: return result

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let update = ready!(self.inner.poll_next_unpin(cx));
        if let Some(DiscoveryUpdateV5::V5(discv5::Event::SessionEstablished(ref enr, _))) = update {
            //
            // Notify discv4 that a discv5 session has been established.
            //
            // A peer with a WAN reachable socket, is likely to make it into discv5 kbuckets
            // shortly after session establishment. Manually added nodes (e.g. from dns service)
            // won't emit a `discv5::Event::NodeInserted`, hence we use the
            // `discv5::Event::SessionEstablished` event + check the enr for contactable address,
            // to determine if discv4 should be notified.
            //
            if discv5::IpMode::Ip4.get_contactable_addr(enr).is_none() &&
                discv5::IpMode::Ip6.get_contactable_addr(enr).is_none()
            {
                cx.waker().wake_by_ref();
                return Poll::Pending
            }
            // todo: get clarity on rules on fork id in discv4
            // todo: check discv4s policy for peers with non-WAN-reachable node records.

            if let Err(err) = self.notify_discv4_of_kbuckets_update() {
                error!(target: "net::discv5",
                    "failed to notify discv4 of discv5 kbuckets update, {err}",
                );
            }
        }

        Poll::Ready(update)
    }
}
