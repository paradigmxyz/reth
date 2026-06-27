//! A dedicated `eth` + `snap/2` stream.
//!
//! [`EthSnapStream`] carries `eth` as the primary protocol and `snap/2` (EIP-8189) as a typed
//! side-channel over a single `RLPx` connection. It owns the raw [`P2PStream`] directly and reuses
//! the transport-free [`EthStreamInner`] codec for `eth` framing, demultiplexing the two protocols
//! by capability message-id offset.

use crate::{
    capability::SharedCapabilities,
    errors::{EthStreamError, P2PStreamError},
    handshake::EthRlpxHandshake,
    message::EthBroadcastMessage,
    Capability, EthMessage, EthNetworkPrimitives, EthStreamInner, EthVersion, NetworkPrimitives,
    P2PStream, UnifiedStatus, HANDSHAKE_TIMEOUT,
};
use alloy_primitives::bytes::{Bytes, BytesMut};
use futures::{Sink, SinkExt, Stream, StreamExt};
use reth_eth_wire_types::{
    snap::{SnapProtocolMessage, SnapVersion},
    RawCapabilityMessage,
};
use reth_ethereum_forks::ForkFilter;
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

/// A dedicated stream that carries `eth` as the primary protocol and `snap/2` (EIP-8189) as a typed
/// side-channel on the same `RLPx` connection.
///
/// Both protocols are exposed through the [`Stream`]/[`Sink`] impls, which yield and accept
/// [`EthSnapMessage`] (an `eth` message or a `snap/2` message). A single poll surfaces whichever
/// protocol the next inbound frame belongs to, so the owner drives one stream rather than two.
#[derive(Debug)]
pub struct EthSnapStream<St, N: NetworkPrimitives = EthNetworkPrimitives> {
    /// The raw `RLPx` stream carrying both `eth` and `snap/2` frames.
    conn: P2PStream<St>,
    /// Transport-free `eth` codec, reused from [`EthStream`](crate::EthStream).
    eth: EthStreamInner<N>,
    /// Relative message-id offset of the `snap/2` capability in the combined message space. Ids at
    /// or above it are snap, rebased to snap-relative and validated by
    /// [`SnapProtocolMessage::decode_versioned`].
    snap_offset: u8,
}

impl<St, N> EthSnapStream<St, N>
where
    St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin + Send + Sync,
    N: NetworkPrimitives,
{
    /// Performs the `eth` status handshake over a connection that negotiated both `eth` and
    /// `snap/2`, returning the established [`EthSnapStream`] and the remote's status.
    ///
    /// The handshake runs directly on the underlying [`P2PStream`]; a `snap/2` message arriving
    /// before the status exchange completes is a protocol violation and surfaces as a handshake
    /// error.
    pub async fn handshake(
        mut conn: P2PStream<St>,
        status: UnifiedStatus,
        fork_filter: ForkFilter,
        handshake: Arc<dyn EthRlpxHandshake>,
        eth_max_message_size: usize,
    ) -> Result<(Self, UnifiedStatus), EthStreamError> {
        let eth_version = conn.shared_capabilities().eth_version()?;
        let snap_offset = eth_snap_layout(conn.shared_capabilities())?;

        let their_status =
            handshake.handshake(&mut conn, status, fork_filter, HANDSHAKE_TIMEOUT).await?;

        let eth = EthStreamInner::with_max_message_size(eth_version, eth_max_message_size);
        Ok((Self { conn, eth, snap_offset }, their_status))
    }
}

impl<St, N: NetworkPrimitives> EthSnapStream<St, N> {
    /// Returns the negotiated `eth` version.
    #[inline]
    pub const fn version(&self) -> EthVersion {
        self.eth.version()
    }

    /// Sets whether to reject block announcement messages before RLP decoding.
    #[inline]
    pub const fn set_reject_block_announcements(&mut self, reject: bool) {
        self.eth.set_reject_block_announcements(reject);
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

impl<St, N> EthSnapStream<St, N>
where
    St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    N: NetworkPrimitives,
{
    /// Queues an [`EthBroadcastMessage`] to be sent on the wire.
    pub fn start_send_broadcast(
        &mut self,
        item: EthBroadcastMessage<N>,
    ) -> Result<(), EthStreamError> {
        self.conn.start_send_unpin(self.eth.encode_broadcast(item)).map_err(Into::into)
    }

    /// Sends a raw capability message over the connection.
    pub fn start_send_raw(&mut self, msg: RawCapabilityMessage) -> Result<(), EthStreamError> {
        self.conn.start_send_unpin(self.eth.encode_raw(msg)).map_err(Into::into)
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

impl<St, N> Stream for EthSnapStream<St, N>
where
    St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    N: NetworkPrimitives,
{
    type Item = Result<EthSnapMessage<N>, EthStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let mut bytes = match ready!(this.conn.poll_next_unpin(cx)) {
            Some(Ok(bytes)) => bytes,
            Some(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
            None => return Poll::Ready(None),
        };

        let Some(&id) = bytes.first() else {
            return Poll::Ready(Some(Err(P2PStreamError::EmptyProtocolMessage.into())))
        };

        // `eth` occupies ids below the snap offset and is decoded by the shared codec. Ids at or
        // above it are snap: rebased to snap-relative (`0x00..`) and validated by
        // `decode_versioned`, which rejects ids that are out of range or invalid for `snap/2`.
        let Some(snap_id) = id.checked_sub(this.snap_offset) else {
            return Poll::Ready(Some(this.eth.decode_message(bytes).map(EthSnapMessage::Eth)))
        };
        bytes[0] = snap_id;
        Poll::Ready(Some(
            SnapProtocolMessage::decode_versioned(SnapVersion::V2, &bytes)
                .map(EthSnapMessage::Snap)
                .map_err(Into::into),
        ))
    }
}

impl<St, N> Sink<EthSnapMessage<N>> for EthSnapStream<St, N>
where
    St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    N: NetworkPrimitives,
{
    type Error = EthStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.get_mut().conn.poll_ready_unpin(cx).map_err(Into::into)
    }

    fn start_send(self: Pin<&mut Self>, item: EthSnapMessage<N>) -> Result<(), Self::Error> {
        let this = self.get_mut();
        let bytes = match item {
            EthSnapMessage::Eth(msg) => this.eth.encode_message(msg)?,
            EthSnapMessage::Snap(msg) => {
                // Reclaim the freshly-encoded buffer as mutable without copying the payload, then
                // rebase the snap-relative id into the combined message space.
                let mut buf =
                    msg.encode().0.try_into_mut().unwrap_or_else(|b| BytesMut::from(b.as_ref()));
                mask_snap(&mut buf, this.snap_offset)?;
                buf.freeze()
            }
        };
        this.conn.start_send_unpin(bytes).map_err(Into::into)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.get_mut().conn.poll_flush_unpin(cx).map_err(Into::into)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.get_mut().conn.poll_close_unpin(cx).map_err(Into::into)
    }
}

/// Resolves the `snap/2` message-id offset from the negotiated capabilities, accepting only a
/// connection that shares exactly `eth` at relative offset 0 immediately followed by `snap/2`.
///
/// The stream forwards every combined id below the snap offset to the eth codec and treats every
/// id at or above it as snap, so any other shared capability before `eth`, between `eth` and
/// `snap/2`, or after `snap/2` would be mis-routed. Such layouts belong on the general-purpose
/// satellite multiplexer and are rejected here.
fn eth_snap_layout(caps: &SharedCapabilities) -> Result<u8, EthStreamError> {
    let snap = caps
        .ensure_matching_capability(&Capability::snap_2())
        .map_err(|_| EthStreamError::from(P2PStreamError::CapabilityNotShared))?;
    let snap_offset = snap.relative_message_id_offset();

    let eth = caps.eth()?;
    if caps.len() != 2 || eth.relative_message_id_offset() != 0 || eth.num_messages() != snap_offset
    {
        return Err(P2PStreamError::CapabilityNotShared.into())
    }
    Ok(snap_offset)
}

/// Rebases a snap-relative message id to the combined message space.
#[inline]
fn mask_snap(bytes: &mut BytesMut, snap_offset: u8) -> Result<(), io::Error> {
    let id = bytes.first().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    bytes[0] =
        id.checked_add(snap_offset).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        handshake::EthHandshake,
        message::MAX_MESSAGE_SIZE,
        protocol::Protocol,
        test_utils::{connect_passthrough, eth_handshake, eth_hello},
        UnauthedP2PStream,
    };
    use reth_eth_wire_types::{
        snap::{BlockAccessListsMessage, GetBlockAccessListsMessage},
        EthVersion,
    };
    use tokio::net::TcpListener;
    use tokio_util::codec::Decoder;

    /// Builds shared capabilities from matching local protocols and peer capabilities.
    fn shared_caps(local: Vec<Protocol>, peer: Vec<Capability>) -> SharedCapabilities {
        SharedCapabilities::try_new(local, peer).unwrap()
    }

    #[test]
    fn eth_snap_layout_accepts_eth_then_snap() {
        let caps = shared_caps(
            vec![EthVersion::Eth68.into(), Protocol::snap_2()],
            vec![EthVersion::Eth68.into(), Capability::snap_2()],
        );
        let offset = eth_snap_layout(&caps).unwrap();
        assert_eq!(offset, caps.eth().unwrap().num_messages());
    }

    #[test]
    fn eth_snap_layout_rejects_missing_snap() {
        let caps = shared_caps(vec![EthVersion::Eth68.into()], vec![EthVersion::Eth68.into()]);
        assert!(eth_snap_layout(&caps).is_err());
    }

    #[test]
    fn eth_snap_layout_rejects_capability_before_eth() {
        // "aaa" sorts before "eth", so it takes relative offset 0 and eth no longer starts at 0.
        let cap = Capability::new_static("aaa", 1);
        let caps = shared_caps(
            vec![Protocol::new(cap.clone(), 5), EthVersion::Eth68.into(), Protocol::snap_2()],
            vec![cap, EthVersion::Eth68.into(), Capability::snap_2()],
        );
        assert!(eth_snap_layout(&caps).is_err());
    }

    #[test]
    fn eth_snap_layout_rejects_capability_between_eth_and_snap() {
        // "les" sorts between "eth" and "snap", leaving a gap so snap no longer directly follows
        // eth.
        let cap = Capability::new_static("les", 1);
        let caps = shared_caps(
            vec![EthVersion::Eth68.into(), Protocol::new(cap.clone(), 5), Protocol::snap_2()],
            vec![EthVersion::Eth68.into(), cap, Capability::snap_2()],
        );
        assert!(eth_snap_layout(&caps).is_err());
    }

    #[test]
    fn eth_snap_layout_rejects_capability_after_snap() {
        // "zzz" sorts after "snap"; eth+snap still line up, but its frames would be mis-routed as
        // snap, so the layout must be rejected (it belongs on the satellite multiplexer).
        let cap = Capability::new_static("zzz", 1);
        let caps = shared_caps(
            vec![EthVersion::Eth68.into(), Protocol::snap_2(), Protocol::new(cap.clone(), 5)],
            vec![EthVersion::Eth68.into(), Capability::snap_2(), cap],
        );
        assert!(eth_snap_layout(&caps).is_err());
    }

    #[test]
    fn mask_snap_rebases_into_combined_space() {
        let mut bytes = BytesMut::from(&[8u8, 0xcc][..]);
        mask_snap(&mut bytes, 17).unwrap();
        assert_eq!(bytes[0], 17 + 8);
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
            let server_hello = eth_snap_hello();
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
        let conn = connect_passthrough(local_addr, eth_snap_hello()).await;
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

    /// Builds a hello advertising `eth` + `snap/2`.
    fn eth_snap_hello() -> crate::HelloMessageWithProtocols {
        let mut hello = eth_hello().0;
        hello.try_add_protocol(Protocol::snap_2()).unwrap();
        hello
    }
}
