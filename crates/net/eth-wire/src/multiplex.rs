//! Rlpx protocol multiplexer and satellite stream
//!
//! A Satellite is a Stream that primarily drives a single RLPx subprotocol but can also handle
//! additional subprotocols.
//!
//! Most of other subprotocols are "dependent satellite" protocols of "eth" and not a fully standalone protocol, for example "snap", See also [snap protocol](https://github.com/ethereum/devp2p/blob/298d7a77c3bf833641579ecbbb5b13f0311eeeea/caps/snap.md?plain=1#L71)
//! Hence it is expected that the primary protocol is "eth" and the additional protocols are
//! "dependent satellite" protocols.

use std::{
    collections::VecDeque,
    fmt,
    future::Future,
    io,
    pin::Pin,
    task::{ready, Context, Poll},
};

use crate::{
    capability::{Capability, SharedCapabilities, SharedCapability, UnsupportedCapabilityError},
    errors::{EthStreamError, P2PStreamError},
    CanDisconnect, DisconnectReason, EthStream, P2PStream, Status, UnauthedEthStream,
};
use bytes::{Bytes, BytesMut};
use futures::{pin_mut, Sink, SinkExt, Stream, StreamExt, TryStream, TryStreamExt};
use reth_primitives::ForkFilter;
use tokio::sync::{mpsc, mpsc::UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A Stream and Sink type that wraps a raw rlpx stream [P2PStream] and handles message ID
/// multiplexing.
#[derive(Debug)]
pub struct RlpxProtocolMultiplexer<St> {
    inner: MultiplexInner<St>,
}

impl<St> RlpxProtocolMultiplexer<St> {
    /// Wraps the raw p2p stream
    pub fn new(conn: P2PStream<St>) -> Self {
        Self {
            inner: MultiplexInner {
                conn,
                protocols: Default::default(),
                out_buffer: Default::default(),
            },
        }
    }

    /// Installs a new protocol on top of the raw p2p stream.
    ///
    /// This accepts a closure that receives a [ProtocolConnection] that will yield messages for the
    /// given capability.
    pub fn install_protocol<F, Proto>(
        &mut self,
        cap: &Capability,
        f: F,
    ) -> Result<(), UnsupportedCapabilityError>
    where
        F: FnOnce(ProtocolConnection) -> Proto,
        Proto: Stream<Item = BytesMut> + Send + 'static,
    {
        self.inner.install_protocol(cap, f)
    }

    /// Returns the [SharedCapabilities] of the underlying raw p2p stream
    pub fn shared_capabilities(&self) -> &SharedCapabilities {
        self.inner.shared_capabilities()
    }

    /// Converts this multiplexer into a [RlpxSatelliteStream] with the given primary protocol.
    pub fn into_satellite_stream<F, Primary>(
        self,
        cap: &Capability,
        primary: F,
    ) -> Result<RlpxSatelliteStream<St, Primary>, P2PStreamError>
    where
        F: FnOnce(ProtocolProxy) -> Primary,
    {
        let Ok(shared_cap) = self.shared_capabilities().ensure_matching_capability(cap).cloned()
        else {
            return Err(P2PStreamError::CapabilityNotShared)
        };

        let (to_primary, from_wire) = mpsc::unbounded_channel();
        let (to_wire, from_primary) = mpsc::unbounded_channel();
        let proxy = ProtocolProxy {
            shared_cap: shared_cap.clone(),
            from_wire: UnboundedReceiverStream::new(from_wire),
            to_wire,
        };

        let st = primary(proxy);
        Ok(RlpxSatelliteStream {
            inner: self.inner,
            primary: PrimaryProtocol {
                to_primary,
                from_primary: UnboundedReceiverStream::new(from_primary),
                st,
                shared_cap,
            },
        })
    }

    /// Converts this multiplexer into a [RlpxSatelliteStream] with the given primary protocol.
    ///
    /// Returns an error if the primary protocol is not supported by the remote or the handshake
    /// failed.
    pub async fn into_satellite_stream_with_handshake<F, Fut, Err, Primary>(
        self,
        cap: &Capability,
        handshake: F,
    ) -> Result<RlpxSatelliteStream<St, Primary>, Err>
    where
        F: FnOnce(ProtocolProxy) -> Fut,
        Fut: Future<Output = Result<Primary, Err>>,
        St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
        P2PStreamError: Into<Err>,
    {
        self.into_satellite_stream_with_tuple_handshake(cap, move |proxy| async move {
            let st = handshake(proxy).await?;
            Ok((st, ()))
        })
        .await
        .map(|(st, _)| st)
    }

    /// Converts this multiplexer into a [RlpxSatelliteStream] with the given primary protocol.
    ///
    /// Returns an error if the primary protocol is not supported by the remote or the handshake
    /// failed.
    ///
    /// This accepts a closure that does a handshake with the remote peer and returns a tuple of the
    /// primary stream and extra data.
    ///
    /// See also [UnauthedEthStream::handshake]
    pub async fn into_satellite_stream_with_tuple_handshake<F, Fut, Err, Primary, Extra>(
        mut self,
        cap: &Capability,
        handshake: F,
    ) -> Result<(RlpxSatelliteStream<St, Primary>, Extra), Err>
    where
        F: FnOnce(ProtocolProxy) -> Fut,
        Fut: Future<Output = Result<(Primary, Extra), Err>>,
        St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
        P2PStreamError: Into<Err>,
    {
        let Ok(shared_cap) = self.shared_capabilities().ensure_matching_capability(cap).cloned()
        else {
            return Err(P2PStreamError::CapabilityNotShared.into())
        };

        let (to_primary, from_wire) = mpsc::unbounded_channel();
        let (to_wire, mut from_primary) = mpsc::unbounded_channel();
        let proxy = ProtocolProxy {
            shared_cap: shared_cap.clone(),
            from_wire: UnboundedReceiverStream::new(from_wire),
            to_wire,
        };

        let f = handshake(proxy);
        pin_mut!(f);

        // this polls the connection and the primary stream concurrently until the handshake is
        // complete
        loop {
            tokio::select! {
                Some(Ok(msg)) = self.inner.conn.next() => {
                    // Ensure the message belongs to the primary protocol
                    let Some(offset) = msg.first().copied()
                    else {
                        return Err(P2PStreamError::EmptyProtocolMessage.into())
                    };
                    if let Some(cap) = self.shared_capabilities().find_by_relative_offset(offset).cloned() {
                            if cap == shared_cap {
                                // delegate to primary
                                let _ = to_primary.send(msg);
                            } else {
                                // delegate to satellite
                                self.inner.delegate_message(&cap, msg);
                            }
                        } else {
                           return Err(P2PStreamError::UnknownReservedMessageId(offset).into())
                        }
                }
                Some(msg) = from_primary.recv() => {
                    self.inner.conn.send(msg).await.map_err(Into::into)?;
                }
                res = &mut f => {
                    let (st, extra) = res?;
                    return Ok((RlpxSatelliteStream {
                            inner: self.inner,
                            primary: PrimaryProtocol {
                                to_primary,
                                from_primary: UnboundedReceiverStream::new(from_primary),
                                st,
                                shared_cap,
                            }
                    }, extra))
                }
            }
        }
    }

    /// Converts this multiplexer into a [RlpxSatelliteStream] with eth protocol as the given
    /// primary protocol.
    pub async fn into_eth_satellite_stream(
        self,
        status: Status,
        fork_filter: ForkFilter,
    ) -> Result<(RlpxSatelliteStream<St, EthStream<ProtocolProxy>>, Status), EthStreamError>
    where
        St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    {
        let eth_cap = self.inner.conn.shared_capabilities().eth_version()?;
        self.into_satellite_stream_with_tuple_handshake(
            &Capability::eth(eth_cap),
            move |proxy| async move {
                UnauthedEthStream::new(proxy).handshake(status, fork_filter).await
            },
        )
        .await
    }
}

#[derive(Debug)]
struct MultiplexInner<St> {
    /// The raw p2p stream
    conn: P2PStream<St>,
    /// All the subprotocols that are multiplexed on top of the raw p2p stream
    protocols: Vec<ProtocolStream>,
    /// Buffer for outgoing messages on the wire.
    out_buffer: VecDeque<Bytes>,
}

impl<St> MultiplexInner<St> {
    fn shared_capabilities(&self) -> &SharedCapabilities {
        self.conn.shared_capabilities()
    }

    /// Delegates a message to the matching protocol.
    fn delegate_message(&mut self, cap: &SharedCapability, msg: BytesMut) -> bool {
        for proto in &self.protocols {
            if proto.shared_cap == *cap {
                proto.send_raw(msg);
                return true
            }
        }
        false
    }

    fn install_protocol<F, Proto>(
        &mut self,
        cap: &Capability,
        f: F,
    ) -> Result<(), UnsupportedCapabilityError>
    where
        F: FnOnce(ProtocolConnection) -> Proto,
        Proto: Stream<Item = BytesMut> + Send + 'static,
    {
        let shared_cap =
            self.conn.shared_capabilities().ensure_matching_capability(cap).cloned()?;
        let (to_satellite, rx) = mpsc::unbounded_channel();
        let proto_conn = ProtocolConnection { from_wire: UnboundedReceiverStream::new(rx) };
        let st = f(proto_conn);
        let st = ProtocolStream { shared_cap, to_satellite, satellite_st: Box::pin(st) };
        self.protocols.push(st);
        Ok(())
    }
}

/// Represents a protocol in the multiplexer that is used as the primary protocol.
#[derive(Debug)]
struct PrimaryProtocol<Primary> {
    /// Channel to send messages to the primary protocol.
    to_primary: UnboundedSender<BytesMut>,
    /// Receiver for messages from the primary protocol.
    from_primary: UnboundedReceiverStream<Bytes>,
    /// Shared capability of the primary protocol.
    shared_cap: SharedCapability,
    /// The primary stream.
    st: Primary,
}

/// A Stream and Sink type that acts as a wrapper around a primary RLPx subprotocol (e.g. "eth")
///
/// Only emits and sends _non-empty_ messages
#[derive(Debug)]
pub struct ProtocolProxy {
    shared_cap: SharedCapability,
    /// Receives _non-empty_ messages from the wire
    from_wire: UnboundedReceiverStream<BytesMut>,
    /// Sends _non-empty_ messages from the wire
    to_wire: UnboundedSender<Bytes>,
}

impl ProtocolProxy {
    /// Sends a _non-empty_ message on the wire.
    fn try_send(&self, msg: Bytes) -> Result<(), io::Error> {
        if msg.is_empty() {
            // message must not be empty
            return Err(io::ErrorKind::InvalidInput.into())
        }
        self.to_wire.send(self.mask_msg_id(msg)?).map_err(|_| io::ErrorKind::BrokenPipe.into())
    }

    /// Masks the message ID of a message to be sent on the wire.
    #[inline]
    fn mask_msg_id(&self, msg: Bytes) -> Result<Bytes, io::Error> {
        if msg.is_empty() {
            // message must not be empty
            return Err(io::ErrorKind::InvalidInput.into())
        }

        let mut masked_bytes = BytesMut::zeroed(msg.len());
        masked_bytes[0] = msg[0]
            .checked_add(self.shared_cap.relative_message_id_offset())
            .ok_or(io::ErrorKind::InvalidInput)?;

        masked_bytes[1..].copy_from_slice(&msg[1..]);
        Ok(masked_bytes.freeze())
    }

    /// Unmasks the message ID of a message received from the wire.
    #[inline]
    fn unmask_id(&self, mut msg: BytesMut) -> Result<BytesMut, io::Error> {
        if msg.is_empty() {
            // message must not be empty
            return Err(io::ErrorKind::InvalidInput.into())
        }
        msg[0] = msg[0]
            .checked_sub(self.shared_cap.relative_message_id_offset())
            .ok_or(io::ErrorKind::InvalidInput)?;
        Ok(msg)
    }
}

impl Stream for ProtocolProxy {
    type Item = Result<BytesMut, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let msg = ready!(self.from_wire.poll_next_unpin(cx));
        Poll::Ready(msg.map(|msg| self.get_mut().unmask_id(msg)))
    }
}

impl Sink<Bytes> for ProtocolProxy {
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        self.get_mut().try_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

impl CanDisconnect<Bytes> for ProtocolProxy {
    async fn disconnect(
        &mut self,
        _reason: DisconnectReason,
    ) -> Result<(), <Self as Sink<Bytes>>::Error> {
        // TODO handle disconnects
        Ok(())
    }
}

/// A connection channel to receive _non_empty_ messages for the negotiated protocol.
///
/// This is a [Stream] that returns raw bytes of the received messages for this protocol.
#[derive(Debug)]
pub struct ProtocolConnection {
    from_wire: UnboundedReceiverStream<BytesMut>,
}

impl Stream for ProtocolConnection {
    type Item = BytesMut;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.from_wire.poll_next_unpin(cx)
    }
}

/// A Stream and Sink type that acts as a wrapper around a primary RLPx subprotocol (e.g. "eth")
/// [EthStream] and can also handle additional subprotocols.
#[derive(Debug)]
pub struct RlpxSatelliteStream<St, Primary> {
    inner: MultiplexInner<St>,
    primary: PrimaryProtocol<Primary>,
}

impl<St, Primary> RlpxSatelliteStream<St, Primary> {
    /// Installs a new protocol on top of the raw p2p stream.
    ///
    /// This accepts a closure that receives a [ProtocolConnection] that will yield messages for the
    /// given capability.
    pub fn install_protocol<F, Proto>(
        &mut self,
        cap: &Capability,
        f: F,
    ) -> Result<(), UnsupportedCapabilityError>
    where
        F: FnOnce(ProtocolConnection) -> Proto,
        Proto: Stream<Item = BytesMut> + Send + 'static,
    {
        self.inner.install_protocol(cap, f)
    }

    /// Returns the primary protocol.
    #[inline]
    pub fn primary(&self) -> &Primary {
        &self.primary.st
    }

    /// Returns mutable access to the primary protocol.
    #[inline]
    pub fn primary_mut(&mut self) -> &mut Primary {
        &mut self.primary.st
    }

    /// Returns the underlying [P2PStream].
    #[inline]
    pub fn inner(&self) -> &P2PStream<St> {
        &self.inner.conn
    }

    /// Returns mutable access to the underlying [P2PStream].
    #[inline]
    pub fn inner_mut(&mut self) -> &mut P2PStream<St> {
        &mut self.inner.conn
    }

    /// Consumes this type and returns the wrapped [P2PStream].
    #[inline]
    pub fn into_inner(self) -> P2PStream<St> {
        self.inner.conn
    }
}

impl<St, Primary, PrimaryErr> Stream for RlpxSatelliteStream<St, Primary>
where
    St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    Primary: TryStream<Error = PrimaryErr> + Unpin,
    P2PStreamError: Into<PrimaryErr>,
{
    type Item = Result<Primary::Ok, Primary::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            // first drain the primary stream
            if let Poll::Ready(Some(msg)) = this.primary.st.try_poll_next_unpin(cx) {
                return Poll::Ready(Some(msg))
            }

            let mut conn_ready = true;
            loop {
                match this.inner.conn.poll_ready_unpin(cx) {
                    Poll::Ready(_) => {
                        if let Some(msg) = this.inner.out_buffer.pop_front() {
                            if let Err(err) = this.inner.conn.start_send_unpin(msg) {
                                return Poll::Ready(Some(Err(err.into())))
                            }
                        } else {
                            break
                        }
                    }
                    Poll::Pending => {
                        conn_ready = false;
                        break
                    }
                }
            }

            // advance primary out
            loop {
                match this.primary.from_primary.poll_next_unpin(cx) {
                    Poll::Ready(Some(msg)) => {
                        this.inner.out_buffer.push_back(msg);
                    }
                    Poll::Ready(None) => {
                        // primary closed
                        return Poll::Ready(None)
                    }
                    Poll::Pending => break,
                }
            }

            // advance all satellites
            for idx in (0..this.inner.protocols.len()).rev() {
                let mut proto = this.inner.protocols.swap_remove(idx);
                loop {
                    match proto.poll_next_unpin(cx) {
                        Poll::Ready(Some(Err(err))) => {
                            return Poll::Ready(Some(Err(P2PStreamError::Io(err).into())))
                        }
                        Poll::Ready(Some(Ok(msg))) => {
                            this.inner.out_buffer.push_back(msg);
                        }
                        Poll::Ready(None) => return Poll::Ready(None),
                        Poll::Pending => {
                            this.inner.protocols.push(proto);
                            break
                        }
                    }
                }
            }

            let mut delegated = false;
            loop {
                // pull messages from connection
                match this.inner.conn.poll_next_unpin(cx) {
                    Poll::Ready(Some(Ok(msg))) => {
                        delegated = true;
                        let Some(offset) = msg.first().copied() else {
                            return Poll::Ready(Some(Err(
                                P2PStreamError::EmptyProtocolMessage.into()
                            )))
                        };
                        // delegate the multiplexed message to the correct protocol
                        if let Some(cap) =
                            this.inner.conn.shared_capabilities().find_by_relative_offset(offset)
                        {
                            if cap == &this.primary.shared_cap {
                                // delegate to primary
                                let _ = this.primary.to_primary.send(msg);
                            } else {
                                // delegate to installed satellite if any
                                for proto in &this.inner.protocols {
                                    if proto.shared_cap == *cap {
                                        proto.send_raw(msg);
                                        break
                                    }
                                }
                            }
                        } else {
                            return Poll::Ready(Some(Err(P2PStreamError::UnknownReservedMessageId(
                                offset,
                            )
                            .into())))
                        }
                    }
                    Poll::Ready(Some(Err(err))) => return Poll::Ready(Some(Err(err.into()))),
                    Poll::Ready(None) => {
                        // connection closed
                        return Poll::Ready(None)
                    }
                    Poll::Pending => break,
                }
            }

            if !conn_ready || (!delegated && this.inner.out_buffer.is_empty()) {
                return Poll::Pending
            }
        }
    }
}

impl<St, Primary, T> Sink<T> for RlpxSatelliteStream<St, Primary>
where
    St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    Primary: Sink<T> + Unpin,
    P2PStreamError: Into<<Primary as Sink<T>>::Error>,
{
    type Error = <Primary as Sink<T>>::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        if let Err(err) = ready!(this.inner.conn.poll_ready_unpin(cx)) {
            return Poll::Ready(Err(err.into()))
        }
        if let Err(err) = ready!(this.primary.st.poll_ready_unpin(cx)) {
            return Poll::Ready(Err(err))
        }
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        self.get_mut().primary.st.start_send_unpin(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.get_mut().inner.conn.poll_flush_unpin(cx).map_err(Into::into)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.get_mut().inner.conn.poll_close_unpin(cx).map_err(Into::into)
    }
}

/// Wraps a RLPx subprotocol and handles message ID multiplexing.
struct ProtocolStream {
    shared_cap: SharedCapability,
    /// the channel shared with the satellite stream
    to_satellite: UnboundedSender<BytesMut>,
    satellite_st: Pin<Box<dyn Stream<Item = BytesMut> + Send>>,
}

impl ProtocolStream {
    /// Masks the message ID of a message to be sent on the wire.
    #[inline]
    fn mask_msg_id(&self, mut msg: BytesMut) -> Result<Bytes, io::Error> {
        if msg.is_empty() {
            // message must not be empty
            return Err(io::ErrorKind::InvalidInput.into())
        }
        msg[0] = msg[0]
            .checked_add(self.shared_cap.relative_message_id_offset())
            .ok_or(io::ErrorKind::InvalidInput)?;
        Ok(msg.freeze())
    }

    /// Unmasks the message ID of a message received from the wire.
    #[inline]
    fn unmask_id(&self, mut msg: BytesMut) -> Result<BytesMut, io::Error> {
        if msg.is_empty() {
            // message must not be empty
            return Err(io::ErrorKind::InvalidInput.into())
        }
        msg[0] = msg[0]
            .checked_sub(self.shared_cap.relative_message_id_offset())
            .ok_or(io::ErrorKind::InvalidInput)?;
        Ok(msg)
    }

    /// Sends the message to the satellite stream.
    fn send_raw(&self, msg: BytesMut) {
        let _ = self.unmask_id(msg).map(|msg| self.to_satellite.send(msg));
    }
}

impl Stream for ProtocolStream {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let msg = ready!(this.satellite_st.as_mut().poll_next(cx));
        Poll::Ready(msg.map(|msg| this.mask_msg_id(msg)))
    }
}

impl fmt::Debug for ProtocolStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtocolStream").field("cap", &self.shared_cap).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{
            connect_passthrough, eth_handshake, eth_hello,
            proto::{test_hello, TestProtoMessage},
        },
        UnauthedP2PStream,
    };
    use tokio::{net::TcpListener, sync::oneshot};
    use tokio_util::codec::Decoder;

    #[tokio::test]
    async fn eth_satellite() {
        reth_tracing::init_test_tracing();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let (status, fork_filter) = eth_handshake();
        let other_status = status;
        let other_fork_filter = fork_filter.clone();
        let _handle = tokio::spawn(async move {
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = crate::PassthroughCodec::default().framed(incoming);
            let (server_hello, _) = eth_hello();
            let (p2p_stream, _) =
                UnauthedP2PStream::new(stream).handshake(server_hello).await.unwrap();

            let (_eth_stream, _) = UnauthedEthStream::new(p2p_stream)
                .handshake(other_status, other_fork_filter)
                .await
                .unwrap();

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        });

        let conn = connect_passthrough(local_addr, eth_hello().0).await;
        let eth = conn.shared_capabilities().eth().unwrap().clone();

        let multiplexer = RlpxProtocolMultiplexer::new(conn);
        let _satellite = multiplexer
            .into_satellite_stream_with_handshake(
                eth.capability().as_ref(),
                move |proxy| async move {
                    UnauthedEthStream::new(proxy).handshake(status, fork_filter).await
                },
            )
            .await
            .unwrap();
    }

    /// A test that install a satellite stream eth+test protocol and sends messages between them.
    #[tokio::test(flavor = "multi_thread")]
    async fn eth_test_protocol_satellite() {
        reth_tracing::init_test_tracing();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let (status, fork_filter) = eth_handshake();
        let other_status = status;
        let other_fork_filter = fork_filter.clone();
        let _handle = tokio::spawn(async move {
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = crate::PassthroughCodec::default().framed(incoming);
            let (server_hello, _) = test_hello();
            let (conn, _) = UnauthedP2PStream::new(stream).handshake(server_hello).await.unwrap();

            let (mut st, _their_status) = RlpxProtocolMultiplexer::new(conn)
                .into_eth_satellite_stream(other_status, other_fork_filter)
                .await
                .unwrap();

            st.install_protocol(&TestProtoMessage::capability(), |mut conn| {
                async_stream::stream! {
                    yield TestProtoMessage::ping().encoded();
                    let msg = conn.next().await.unwrap();
                    let msg = TestProtoMessage::decode_message(&mut &msg[..]).unwrap();
                    assert_eq!(msg, TestProtoMessage::pong());

                    yield TestProtoMessage::message("hello").encoded();
                    let msg = conn.next().await.unwrap();
                    let msg = TestProtoMessage::decode_message(&mut &msg[..]).unwrap();
                    assert_eq!(msg, TestProtoMessage::message("good bye!"));

                    yield TestProtoMessage::message("good bye!").encoded();

                    futures::future::pending::<()>().await;
                    unreachable!()
                }
            })
            .unwrap();

            loop {
                let _ = st.next().await;
            }
        });

        let conn = connect_passthrough(local_addr, test_hello().0).await;
        let (mut st, _their_status) = RlpxProtocolMultiplexer::new(conn)
            .into_eth_satellite_stream(status, fork_filter)
            .await
            .unwrap();

        let (tx, mut rx) = oneshot::channel();

        st.install_protocol(&TestProtoMessage::capability(), |mut conn| {
            async_stream::stream! {
                let msg = conn.next().await.unwrap();
                let msg = TestProtoMessage::decode_message(&mut &msg[..]).unwrap();
                assert_eq!(msg, TestProtoMessage::ping());

                yield TestProtoMessage::pong().encoded();

                let msg = conn.next().await.unwrap();
                let msg = TestProtoMessage::decode_message(&mut &msg[..]).unwrap();
                assert_eq!(msg, TestProtoMessage::message("hello"));

                yield TestProtoMessage::message("good bye!").encoded();

                let msg = conn.next().await.unwrap();
                let msg = TestProtoMessage::decode_message(&mut &msg[..]).unwrap();
                assert_eq!(msg, TestProtoMessage::message("good bye!"));

                tx.send(()).unwrap();

                futures::future::pending::<()>().await;
                unreachable!()
            }
        })
        .unwrap();

        loop {
            tokio::select! {
                _ = &mut rx => {
                    break
                }
               _ = st.next() => {
                }
            }
        }
    }
}
