//! A dedicated `eth` + `snap/2` stream.
//!
//! [`EthSnapStream`] carries `eth` as the primary protocol and `snap/2` (EIP-8189) as a typed
//! side-channel over a single `RLPx` connection.

use crate::{
    errors::{EthStreamError, P2PStreamError},
    handshake::EthRlpxHandshake,
    CanDisconnect, Capability, DisconnectReason, EthMessage, EthStream, P2PStream, UnifiedStatus,
    HANDSHAKE_TIMEOUT,
};
use bytes::{Bytes, BytesMut};
use futures::{Sink, SinkExt, Stream, StreamExt};
use reth_eth_wire_types::{
    snap::{SnapProtocolMessage, SnapVersion},
    EthNetworkPrimitives, NetworkPrimitives,
};
use reth_ethereum_forks::ForkFilter;
use std::{
    io,
    pin::{pin, Pin},
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A dedicated stream that carries `eth` as the primary protocol and `snap/2` (EIP-8189) as a typed
/// side-channel on the same `RLPx` connection.
///
/// Both protocols are exposed through the [`Stream`]/[`Sink`] impls, which yield and accept
/// [`EthSnapMessage`] (an `eth` message or a `snap/2` message). A single poll surfaces whichever
/// protocol has data, so the owner drives one stream rather than two.
#[derive(Debug)]
pub struct EthSnapStream<St, N: NetworkPrimitives = EthNetworkPrimitives> {
    /// The raw `RLPx` stream that owns the wire.
    conn: P2PStream<St>,
    /// The `eth` primary, reading/writing `eth` bytes through channels bridged to `conn`.
    eth: EthStream<EthProxy, N>,
    /// Forwards inbound `eth` bytes from the wire to [`Self::eth`].
    to_eth: mpsc::UnboundedSender<BytesMut>,
    /// Outbound `eth` bytes produced by [`Self::eth`], to be framed onto the wire.
    from_eth: UnboundedReceiverStream<Bytes>,
    /// Forwards inbound `snap/2` bytes (snap-relative ids) to the consumer.
    to_snap: mpsc::UnboundedSender<BytesMut>,
    /// Inbound `snap/2` bytes (snap-relative ids) for the consumer to read.
    snap_inbound: UnboundedReceiverStream<BytesMut>,
    /// Outbound `snap/2` bytes (snap-relative ids) queued by the consumer.
    snap_outbound: mpsc::UnboundedSender<BytesMut>,
    /// Outbound `snap/2` bytes (snap-relative ids) to be masked and framed onto the wire.
    from_snap: UnboundedReceiverStream<BytesMut>,
    /// Relative message-id offset of the `snap/2` capability in the combined message space.
    snap_offset: u8,
    /// Number of message ids reserved by the negotiated `snap/2` capability. Combined ids in
    /// `snap_offset..snap_offset + snap_messages` are snap; anything beyond is out of range.
    snap_messages: u8,
}

impl<St, N> EthSnapStream<St, N>
where
    St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin + Send,
    N: NetworkPrimitives,
{
    /// Performs the `eth` status handshake over a connection that negotiated both `eth` and
    /// `snap/2`, returning the established [`EthSnapStream`] and the remote's status.
    ///
    /// `snap/2` messages that arrive during the `eth` handshake are buffered and delivered once the
    /// stream is polled.
    pub async fn handshake(
        mut conn: P2PStream<St>,
        status: UnifiedStatus,
        fork_filter: ForkFilter,
        handshake: Arc<dyn EthRlpxHandshake>,
        eth_max_message_size: usize,
    ) -> Result<(Self, UnifiedStatus), EthStreamError> {
        let eth_version = conn.shared_capabilities().eth_version()?;
        let snap_cap = conn
            .shared_capabilities()
            .ensure_matching_capability(&Capability::snap_2())
            .map_err(|_| P2PStreamError::CapabilityNotShared)?;
        let snap_offset = snap_cap.relative_message_id_offset();
        let snap_messages = snap_cap.num_messages();

        // eth bytes flow conn <-> demux <-> EthProxy <-> EthStream.
        let (to_eth, eth_from_wire) = mpsc::unbounded_channel();
        let (eth_to_wire, mut from_eth) = mpsc::unbounded_channel();
        let proxy = EthProxy {
            from_wire: UnboundedReceiverStream::new(eth_from_wire),
            to_wire: eth_to_wire,
        };

        // snap bytes (snap-relative ids) flow conn <-> demux <-> consumer.
        let (to_snap, snap_inbound) = mpsc::unbounded_channel();
        let (snap_outbound, from_snap) = mpsc::unbounded_channel();

        // Drive the connection and the `eth` handshake concurrently: route inbound messages to the
        // eth proxy or buffer them for snap, and flush the proxy's outbound status messages.
        let mut unauth = UnauthEthProxy { inner: proxy };
        let their_status = {
            let f = handshake.handshake(&mut unauth, status, fork_filter, HANDSHAKE_TIMEOUT);
            let mut f = pin!(f);
            loop {
                tokio::select! {
                    biased;
                    inbound = conn.next() => match inbound {
                        Some(Ok(msg)) => {
                            route_inbound(msg, snap_offset, snap_messages, &to_eth, &to_snap)?;
                        }
                        // Surface a peer error / disconnect immediately instead of waiting for the
                        // handshake to time out.
                        Some(Err(err)) => return Err(err.into()),
                        None => {
                            return Err(P2PStreamError::Disconnected(
                                DisconnectReason::DisconnectRequested,
                            )
                            .into())
                        }
                    },
                    Some(bytes) = from_eth.recv() => {
                        conn.send(bytes).await?;
                    }
                    res = &mut f => break res?,
                }
            }
        };

        let eth = EthStream::with_max_message_size(
            eth_version,
            unauth.into_inner(),
            eth_max_message_size,
        );

        Ok((
            Self {
                conn,
                eth,
                to_eth,
                from_eth: UnboundedReceiverStream::new(from_eth),
                to_snap,
                snap_inbound: UnboundedReceiverStream::new(snap_inbound),
                snap_outbound,
                from_snap: UnboundedReceiverStream::new(from_snap),
                snap_offset,
                snap_messages,
            },
            their_status,
        ))
    }
}

impl<St, N: NetworkPrimitives> EthSnapStream<St, N> {
    /// Returns a reference to the underlying `eth` stream.
    #[inline]
    pub const fn primary(&self) -> &EthStream<EthProxy, N> {
        &self.eth
    }

    /// Returns mutable access to the underlying `eth` stream.
    #[inline]
    pub const fn primary_mut(&mut self) -> &mut EthStream<EthProxy, N> {
        &mut self.eth
    }

    /// Returns a reference to the underlying [`P2PStream`].
    #[inline]
    pub const fn inner(&self) -> &P2PStream<St> {
        &self.conn
    }

    /// Returns mutable access to the underlying [`P2PStream`].
    #[inline]
    pub const fn inner_mut(&mut self) -> &mut P2PStream<St> {
        &mut self.conn
    }

    /// Consumes this type and returns the underlying [`P2PStream`].
    #[inline]
    pub fn into_inner(self) -> P2PStream<St> {
        self.conn
    }
}

/// A message carried by an [`EthSnapStream`]: either an `eth` message or a `snap/2` message.
#[derive(Debug)]
pub enum EthSnapMessage<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// An `eth` protocol message.
    Eth(EthMessage<N>),
    /// A `snap/2` (EIP-8189) protocol message.
    Snap(SnapProtocolMessage),
}

impl<St, N> EthSnapStream<St, N>
where
    St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    N: NetworkPrimitives,
{
    /// Moves queued outbound bytes from the proxy channels onto the wire, `eth` before `snap`.
    ///
    /// Returns `Ready(Ok(()))` once both channels are drained (the wire holds every queued frame),
    /// `Pending` while the wire is not ready to accept more, or an error from the wire. This is the
    /// single place that bridges the proxy channels to `conn`, so [`Sink::poll_flush`] can rely on
    /// it instead of reporting flushed while bytes still sit in the channels.
    fn poll_service_outbound(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), EthStreamError>> {
        loop {
            match self.conn.poll_ready_unpin(cx) {
                Poll::Ready(Ok(())) => {
                    let next = match self.from_eth.poll_next_unpin(cx) {
                        Poll::Ready(Some(bytes)) => Some(bytes),
                        _ => match self.from_snap.poll_next_unpin(cx) {
                            Poll::Ready(Some(mut bytes)) => {
                                mask_snap(&mut bytes, self.snap_offset)?;
                                Some(bytes.freeze())
                            }
                            _ => None,
                        },
                    };
                    match next {
                        Some(bytes) => self.conn.start_send_unpin(bytes)?,
                        None => return Poll::Ready(Ok(())),
                    }
                }
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err.into())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<St, N> Stream for EthSnapStream<St, N>
where
    St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    N: NetworkPrimitives,
{
    type Item = Result<EthSnapMessage<N>, EthStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            // Return any `eth` message the primary already decoded.
            match this.eth.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(msg))) => {
                    return Poll::Ready(Some(Ok(EthSnapMessage::Eth(msg))))
                }
                Poll::Ready(Some(Err(err))) => return Poll::Ready(Some(Err(err))),
                _ => {}
            }

            // Return the next decoded inbound `snap/2` message. An id invalid in snap/2 (the
            // removed trie-node messages `0x06`/`0x07`), an unknown id, or a malformed
            // payload is a protocol violation by the peer and surfaces as a
            // protocol-breach error, not silently dropped.
            if let Poll::Ready(Some(bytes)) = this.snap_inbound.poll_next_unpin(cx) {
                return match SnapProtocolMessage::decode_versioned(SnapVersion::V2, &bytes) {
                    Ok(msg) => Poll::Ready(Some(Ok(EthSnapMessage::Snap(msg)))),
                    Err(err) => Poll::Ready(Some(Err(err.into()))),
                }
            }

            // Move queued outbound (`eth` then `snap`) from the proxy channels onto the wire.
            if let Poll::Ready(Err(err)) = this.poll_service_outbound(cx) {
                return Poll::Ready(Some(Err(err)))
            }

            // Pull inbound messages off the wire and route them by capability.
            let mut delegated = false;
            loop {
                match this.conn.poll_next_unpin(cx) {
                    Poll::Ready(Some(Ok(msg))) => {
                        delegated = true;
                        route_inbound(
                            msg,
                            this.snap_offset,
                            this.snap_messages,
                            &this.to_eth,
                            &this.to_snap,
                        )?;
                    }
                    Poll::Ready(Some(Err(err))) => return Poll::Ready(Some(Err(err.into()))),
                    Poll::Ready(None) => return Poll::Ready(None),
                    Poll::Pending => break,
                }
            }

            // Surface flush errors instead of dropping them.
            if let Poll::Ready(Err(err)) = this.conn.poll_flush_unpin(cx) {
                return Poll::Ready(Some(Err(err.into())))
            }

            // Loop to surface anything we just routed to the primary/snap; otherwise yield.
            if delegated {
                continue
            }
            return Poll::Pending
        }
    }
}

impl<St, N> Sink<EthSnapMessage<N>> for EthSnapStream<St, N>
where
    St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    N: NetworkPrimitives,
{
    type Error = EthStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        if let Err(err) = ready!(this.conn.poll_ready_unpin(cx)) {
            return Poll::Ready(Err(err.into()))
        }
        this.eth.poll_ready_unpin(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: EthSnapMessage<N>) -> Result<(), Self::Error> {
        let this = self.get_mut();
        match item {
            EthSnapMessage::Eth(msg) => this.eth.start_send_unpin(msg),
            EthSnapMessage::Snap(msg) => this
                .snap_outbound
                .send(BytesMut::from(msg.encode().as_ref()))
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe).into()),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        // Push the eth stream's encoded bytes into the proxy channel...
        ready!(this.eth.poll_flush_unpin(cx))?;
        // ...move all queued eth/snap bytes from the proxy channels onto the wire...
        ready!(this.poll_service_outbound(cx))?;
        // ...then flush the wire itself.
        this.conn.poll_flush_unpin(cx).map_err(Into::into)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        // Flush before closing: drain the eth stream and the proxy channels onto the wire first, so
        // a close after start_send/feed cannot silently drop queued eth or snap frames.
        ready!(this.eth.poll_flush_unpin(cx))?;
        ready!(this.poll_service_outbound(cx))?;
        this.conn.poll_close_unpin(cx).map_err(Into::into)
    }
}

/// Routes an inbound combined-id message to the `eth` primary or buffers it for `snap/2`.
///
/// The combined id space is partitioned as `eth = 0..snap_offset` and
/// `snap = snap_offset..snap_offset + snap_messages`. `eth` ids pass through unchanged; `snap` ids
/// are rebased to snap-relative (`0x00..`) before buffering. An id beyond the `snap/2` range is a
/// protocol violation (the connection carries only `eth` and `snap/2`).
fn route_inbound(
    mut msg: BytesMut,
    snap_offset: u8,
    snap_messages: u8,
    to_eth: &mpsc::UnboundedSender<BytesMut>,
    to_snap: &mpsc::UnboundedSender<BytesMut>,
) -> Result<(), EthStreamError> {
    let id = *msg.first().ok_or(P2PStreamError::EmptyProtocolMessage)?;
    if id < snap_offset {
        let _ = to_eth.send(msg);
    } else if id - snap_offset < snap_messages {
        msg[0] = id - snap_offset;
        let _ = to_snap.send(msg);
    } else {
        return Err(P2PStreamError::UnknownReservedMessageId(id).into())
    }
    Ok(())
}

/// Rebases a snap-relative message id to the combined message space.
#[inline]
fn mask_snap(bytes: &mut BytesMut, snap_offset: u8) -> Result<(), io::Error> {
    let id = bytes.first().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    bytes[0] =
        id.checked_add(snap_offset).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    Ok(())
}

/// In-memory transport handed to the inner [`EthStream`] so it frames `eth` messages without owning
/// the shared wire.
///
/// [`EthSnapStream`] owns the `RLPx` connection and demultiplexes it: inbound `eth` bytes are
/// routed into this proxy for [`EthStream`] to decode, and the bytes [`EthStream`] encodes come
/// back out to be framed onto the wire. `eth` is the first shared capability (relative offset `0`),
/// so bytes pass through verbatim with no id masking in either direction.
#[derive(Debug)]
pub struct EthProxy {
    from_wire: UnboundedReceiverStream<BytesMut>,
    to_wire: mpsc::UnboundedSender<Bytes>,
}

impl Stream for EthProxy {
    type Item = Result<BytesMut, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.from_wire.poll_next_unpin(cx).map(|opt| opt.map(Ok))
    }
}

impl Sink<Bytes> for EthProxy {
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        if item.is_empty() {
            return Err(io::ErrorKind::InvalidInput.into())
        }
        self.to_wire.send(item).map_err(|_| io::ErrorKind::BrokenPipe.into())
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

impl CanDisconnect<Bytes> for EthProxy {
    fn disconnect(
        &mut self,
        _reason: DisconnectReason,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), io::Error>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Adapter so the injected [`EthRlpxHandshake`] can run over [`EthProxy`] with the
/// [`P2PStreamError`] error type it expects.
#[derive(Debug)]
struct UnauthEthProxy {
    inner: EthProxy,
}

impl UnauthEthProxy {
    fn into_inner(self) -> EthProxy {
        self.inner
    }
}

impl Stream for UnauthEthProxy {
    type Item = Result<BytesMut, P2PStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.poll_next_unpin(cx).map(|opt| opt.map(|res| res.map_err(P2PStreamError::from)))
    }
}

impl Sink<Bytes> for UnauthEthProxy {
    type Error = P2PStreamError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready_unpin(cx).map_err(P2PStreamError::from)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        self.inner.start_send_unpin(item).map_err(P2PStreamError::from)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_flush_unpin(cx).map_err(P2PStreamError::from)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_close_unpin(cx).map_err(P2PStreamError::from)
    }
}

impl CanDisconnect<Bytes> for UnauthEthProxy {
    fn disconnect(
        &mut self,
        reason: DisconnectReason,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), P2PStreamError>> + Send + '_>> {
        let fut = self.inner.disconnect(reason);
        Box::pin(async move { fut.await.map_err(P2PStreamError::from) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        handshake::EthHandshake,
        message::MAX_MESSAGE_SIZE,
        test_utils::{connect_passthrough, eth_handshake, eth_hello},
        UnauthedP2PStream,
    };
    use reth_eth_wire_types::snap::{BlockAccessListsMessage, GetBlockAccessListsMessage};
    use tokio::net::TcpListener;
    use tokio_util::codec::Decoder;

    // `snap/2` shares the connection right after `eth` (e.g. eth/69 reserves 17 ids), so the snap
    // capability sits at this relative offset in the combined message space and reserves 10 ids.
    const SNAP_OFFSET: u8 = 17;
    const SNAP_MESSAGES: u8 = 10;

    #[test]
    fn routes_inbound_by_capability_offset() {
        let (to_eth, mut eth_rx) = mpsc::unbounded_channel();
        let (to_snap, mut snap_rx) = mpsc::unbounded_channel();

        // An `eth` message (combined id < snap offset) passes through unchanged.
        route_inbound(
            BytesMut::from(&[5u8, 0xaa][..]),
            SNAP_OFFSET,
            SNAP_MESSAGES,
            &to_eth,
            &to_snap,
        )
        .unwrap();
        // A `snap/2` GetBlockAccessLists (0x08) arrives at combined id `SNAP_OFFSET + 8`.
        route_inbound(
            BytesMut::from(&[SNAP_OFFSET + 8, 0xbb][..]),
            SNAP_OFFSET,
            SNAP_MESSAGES,
            &to_eth,
            &to_snap,
        )
        .unwrap();

        assert_eq!(eth_rx.try_recv().unwrap()[0], 5);
        // Rebased to a snap-relative id.
        assert_eq!(snap_rx.try_recv().unwrap()[0], 8);
    }

    #[test]
    fn snap_masking_round_trips_with_routing() {
        // Outbound: a snap-relative id is masked into the combined space...
        let mut bytes = BytesMut::from(&[8u8, 0xcc][..]);
        mask_snap(&mut bytes, SNAP_OFFSET).unwrap();
        assert_eq!(bytes[0], SNAP_OFFSET + 8);

        // ...and inbound routing rebases it back to the same snap-relative id.
        let (to_eth, _eth_rx) = mpsc::unbounded_channel();
        let (to_snap, mut snap_rx) = mpsc::unbounded_channel();
        route_inbound(bytes, SNAP_OFFSET, SNAP_MESSAGES, &to_eth, &to_snap).unwrap();
        assert_eq!(snap_rx.try_recv().unwrap()[0], 8);
    }

    #[test]
    fn route_inbound_rejects_empty_message() {
        let (to_eth, _eth_rx) = mpsc::unbounded_channel();
        let (to_snap, _snap_rx) = mpsc::unbounded_channel();
        assert!(
            route_inbound(BytesMut::new(), SNAP_OFFSET, SNAP_MESSAGES, &to_eth, &to_snap).is_err()
        );
    }

    #[test]
    fn route_inbound_rejects_id_beyond_snap_range() {
        // An id past the `eth` + `snap/2` range cannot belong to this connection.
        let (to_eth, _eth_rx) = mpsc::unbounded_channel();
        let (to_snap, _snap_rx) = mpsc::unbounded_channel();
        let out_of_range = SNAP_OFFSET + SNAP_MESSAGES;
        assert!(route_inbound(
            BytesMut::from(&[out_of_range, 0x00][..]),
            SNAP_OFFSET,
            SNAP_MESSAGES,
            &to_eth,
            &to_snap,
        )
        .is_err());
    }

    /// End-to-end: two peers negotiate `eth` + `snap/2`, then a `GetBlockAccessLists` request and
    /// its `BlockAccessLists` response round-trip over live [`EthSnapStream`]s.
    #[tokio::test(flavor = "multi_thread")]
    async fn snap_request_response_round_trips_over_the_wire() {
        reth_tracing::init_test_tracing();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let (status, fork_filter) = eth_handshake();
        let server_status = status;
        let server_fork_filter = fork_filter.clone();

        // Server: accept, negotiate, and answer one GetBlockAccessLists echoing the request id.
        let server = tokio::spawn(async move {
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = crate::PassthroughCodec::default().framed(incoming);
            let server_hello = eth_hello().0.with_snap(true);
            let (conn, _) = UnauthedP2PStream::new(stream).handshake(server_hello).await.unwrap();

            let (mut stream, _) = EthSnapStream::<_, EthNetworkPrimitives>::handshake(
                conn,
                server_status,
                server_fork_filter,
                Arc::new(EthHandshake::default()),
                MAX_MESSAGE_SIZE,
            )
            .await
            .unwrap();

            while let Some(Ok(msg)) = stream.next().await {
                if let EthSnapMessage::Snap(SnapProtocolMessage::GetBlockAccessLists(req)) = msg {
                    let response = SnapProtocolMessage::BlockAccessLists(BlockAccessListsMessage {
                        request_id: req.request_id,
                        block_access_lists: reth_eth_wire_types::BlockAccessLists(vec![None]),
                    });
                    stream.send(EthSnapMessage::Snap(response)).await.unwrap();
                }
            }
        });

        // Client: connect, negotiate, send the request, and await the correlated response.
        let conn = connect_passthrough(local_addr, eth_hello().0.with_snap(true)).await;
        let (mut stream, _) = EthSnapStream::<_, EthNetworkPrimitives>::handshake(
            conn,
            status,
            fork_filter,
            Arc::new(EthHandshake::default()),
            MAX_MESSAGE_SIZE,
        )
        .await
        .unwrap();

        stream
            .send(EthSnapMessage::Snap(SnapProtocolMessage::GetBlockAccessLists(
                GetBlockAccessListsMessage {
                    request_id: 7,
                    block_hashes: Vec::new(),
                    response_bytes: u64::MAX,
                },
            )))
            .await
            .unwrap();

        let response = loop {
            if let EthSnapMessage::Snap(SnapProtocolMessage::BlockAccessLists(resp)) =
                stream.next().await.unwrap().unwrap()
            {
                break resp;
            }
        };
        assert_eq!(response.request_id, 7);

        server.abort();
    }
}
