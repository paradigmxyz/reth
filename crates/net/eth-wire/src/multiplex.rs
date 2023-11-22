//! Rlpx protocol multiplexer and satellite stream
//!
//! A Satellite is a Stream that primarily drives a single RLPx subprotocol but can also handle
//! additional subprotocols.
//!
//! Most of other subprotocols are "dependent satellite" protocols of "eth" and not a fully standalone protocol, for example "snap", See also [snap protocol](https://github.com/ethereum/devp2p/blob/298d7a77c3bf833641579ecbbb5b13f0311eeeea/caps/snap.md?plain=1#L71)
//! Hence it is expected that the primary protocol is "eth" and the additional protocols are
//! "dependent satellite" protocols.

use crate::{
    capability::{Capability, SharedCapabilities, SharedCapability, UnsupportedCapabilityError},
    CanDisconnect, DisconnectReason, P2PStream,
};
use bytes::{Bytes, BytesMut};
use futures::{Sink, Stream, StreamExt};
use std::{
    fmt,
    future::Future,
    io,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::{mpsc, mpsc::UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A Stream and Sink type that wraps a raw rlpx stream [P2PStream] and handles message ID
/// multiplexing.
#[derive(Debug)]
pub struct RlpxProtocolMultiplexer<St> {
    /// The raw p2p stream
    conn: P2PStream<St>,
    /// All the subprotocols that are multiplexed on top of the raw p2p stream
    protocols: Vec<ProtocolStream>,
}

impl<St> RlpxProtocolMultiplexer<St> {
    /// Wraps the raw p2p stream
    pub fn new(conn: P2PStream<St>) -> Self {
        Self { conn, protocols: Default::default() }
    }

    /// Installs a new protocol on top of the raw p2p stream
    pub fn install_protocol<S>(
        &mut self,
        cap: Capability,
        st: S,
    ) -> Result<(), UnsupportedCapabilityError> {
        todo!()
    }

    /// Returns the [SharedCapabilities] of the underlying raw p2p stream
    pub fn shared_capabilities(&self) -> &SharedCapabilities {
        self.conn.shared_capabilities()
    }

    /// Converts this multiplexer into a [RlpxSatelliteStream] with the given primary protocol.
    ///
    /// Returns an error if the primary protocol is not supported by the remote.
    pub async fn into_satellite_stream<F, Fut, Err, Primary>(
        self,
        cap: &Capability,
        mut f: F,
    ) -> Result<RlpxSatelliteStream<St, Primary>, Self>
    where
        F: FnMut(ProtocolProxy) -> Fut,
        Fut: Future<Output = Result<Primary, Err>>,
    {
        let Ok(shared_cap) = self.shared_capabilities().ensure_matching_capability(cap).cloned()
        else {
            return Err(self)
        };

        let (to_primary, from_wire) = mpsc::unbounded_channel();
        let (to_wire, from_primary) = mpsc::unbounded_channel();
        let proxy = ProtocolProxy {
            shared_cap: shared_cap.clone(),
            from_wire: UnboundedReceiverStream::new(from_wire),
            to_wire,
        };

        let Ok(primary) = f(proxy).await else { return Err(self) };
        let Self { conn, protocols } = self;
        Ok(RlpxSatelliteStream {
            conn,
            to_primary,
            from_primary: UnboundedReceiverStream::new(from_primary),
            primary,
            primary_capability: shared_cap,
            satellites: protocols,
        })
    }
}

#[derive(Debug)]
pub struct ProtocolProxy {
    shared_cap: SharedCapability,
    from_wire: UnboundedReceiverStream<BytesMut>,
    to_wire: UnboundedSender<Bytes>,
}

impl Stream for ProtocolProxy {
    type Item = Result<BytesMut, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let msg = ready!(self.from_wire.poll_next_unpin(cx));
        Poll::Ready(msg.map(Ok))
    }
}

impl Sink<Bytes> for ProtocolProxy {
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        self.to_wire.send(item).map_err(|_| io::ErrorKind::BrokenPipe.into())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

#[async_trait::async_trait]
impl CanDisconnect<Bytes> for ProtocolProxy {
    async fn disconnect(
        &mut self,
        reason: DisconnectReason,
    ) -> Result<(), <Self as Sink<Bytes>>::Error> {
        todo!()
    }
}

/// A connection channel to receive messages for the negotiated protocol.
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
/// [EthStream](crate::EthStream) and can also handle additional subprotocols.
#[derive(Debug)]
pub struct RlpxSatelliteStream<St, Primary> {
    /// The raw p2p stream
    conn: P2PStream<St>,
    to_primary: UnboundedSender<BytesMut>,
    from_primary: UnboundedReceiverStream<Bytes>,
    primary: Primary,
    primary_capability: SharedCapability,
    satellites: Vec<ProtocolStream>,
}

impl<St, Primary> RlpxSatelliteStream<St, Primary> {}

impl<St, Primary> Stream for RlpxSatelliteStream<St, Primary>
where
    St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    Primary: Stream + Unpin,
{
    type Item = <Primary as Stream>::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        todo!()
    }
}

impl<St, Primary, T> Sink<T> for RlpxSatelliteStream<St, Primary>
where
    St: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    Primary: Sink<T, Error = io::Error> + Unpin,
{
    type Error = <Primary as Sink<T>>::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        todo!()
    }

    fn start_send(self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        todo!()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        todo!()
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        todo!()
    }
}

/// Wraps a RLPx subprotocol and handles message ID multiplexing.
struct ProtocolStream {
    cap: SharedCapability,
    /// the channel shared with the satellite stream
    to_satellite: UnboundedSender<BytesMut>,
    satellite_st: Pin<Box<dyn Stream<Item = BytesMut>>>,
}

impl fmt::Debug for ProtocolStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtocolStream").field("cap", &self.cap).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{connect_passthrough, eth_handshake, eth_hello},
        UnauthedEthStream, UnauthedP2PStream,
    };
    use tokio::net::TcpListener;
    use tokio_util::codec::Decoder;

    #[tokio::test]
    async fn eth_satellite() {
        reth_tracing::init_test_tracing();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let (status, fork_filter) = eth_handshake();

        let handle = tokio::spawn(async move {
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = crate::PassthroughCodec::default().framed(incoming);
            let (server_hello, _) = eth_hello();
            let (status, fork_filter) = eth_handshake();
            let (p2p_stream, _) =
                UnauthedP2PStream::new(stream).handshake(server_hello).await.unwrap();

            let (eth_stream, _) =
                UnauthedEthStream::new(p2p_stream).handshake(status, fork_filter).await.unwrap();

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        });

        let conn = connect_passthrough(local_addr, eth_hello().0).await;
        let eth = conn.shared_capabilities().eth().unwrap().clone();
        let multiplexer = RlpxProtocolMultiplexer::new(conn);

        let satellite = multiplexer
            .into_satellite_stream(eth.capability().as_ref(), |proxy| async move {
                UnauthedEthStream::new(proxy).handshake(status, fork_filter).await
            })
            .await
            .unwrap();
    }
}
