//! Connection types for a session

use futures::{Sink, SinkExt, Stream};
use pin_project::pin_project;
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    errors::EthStreamError,
    eth_snap_stream::{EthSnapMessage, EthSnapStream},
    message::EthBroadcastMessage,
    multiplex::{ProtocolProxy, RlpxSatelliteStream},
    EthNetworkPrimitives, EthStream, EthVersion, NetworkPrimitives, P2PStream,
};
use reth_eth_wire_types::RawCapabilityMessage;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::net::TcpStream;

/// The type of the underlying peer network connection.
pub type EthPeerConnection<N> = EthStream<P2PStream<ECIESStream<TcpStream>>, N>;
/// The type of the underlying peer network connection when snap is also shared.
pub type EthSnapPeerConnection<N> = EthSnapStream<P2PStream<ECIESStream<TcpStream>>, N>;

/// Various connection types that at least support the ETH protocol.
pub type EthSatelliteConnection<N = EthNetworkPrimitives> =
    RlpxSatelliteStream<ECIESStream<TcpStream>, EthStream<ProtocolProxy, N>>;
/// Various connection types when snap is also shared (eth + snap combined stream).
pub type EthSnapSatelliteConnection<N = EthNetworkPrimitives> =
    RlpxSatelliteStream<ECIESStream<TcpStream>, EthSnapStream<ProtocolProxy, N>>;

/// Connection types that support the ETH protocol.
///
/// This can be either:
/// - A connection that only supports the ETH protocol
/// - A connection that supports the ETH protocol and at least one other `RLPx` protocol
// This type is boxed because the underlying stream is ~6KB,
// mostly coming from `P2PStream`'s `snap::Encoder` (2072), and `ECIESStream` (3600).
#[pin_project(project = EthRlpxConnProj)]
#[derive(Debug)]
pub enum EthRlpxConnection<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// A connection that only supports the ETH protocol.
    EthOnly(#[pin] Box<EthPeerConnection<N>>),
    /// A connection that supports the ETH protocol and __at least one other__ `RLPx` protocol.
    Satellite(#[pin] Box<EthSatelliteConnection<N>>),
    /// A connection that supports eth + snap combined stream.
    EthSnapOnly(#[pin] Box<EthSnapPeerConnection<N>>),
    /// A connection that supports eth + snap with additional protocols.
    SnapSatellite(#[pin] Box<EthSnapSatelliteConnection<N>>),
}

impl<N: NetworkPrimitives> EthRlpxConnection<N> {
    /// Returns the negotiated ETH version.
    #[inline]
    pub(crate) const fn version(&self) -> EthVersion {
        match self {
            Self::EthOnly(conn) => conn.version(),
            Self::Satellite(conn) => conn.primary().version(),
            Self::EthSnapOnly(conn) => conn.eth_version(),
            Self::SnapSatellite(conn) => conn.primary().eth_version(),
        }
    }

    /// Consumes this type and returns the wrapped [`P2PStream`].
    #[inline]
    pub(crate) fn into_inner(self) -> P2PStream<ECIESStream<TcpStream>> {
        match self {
            Self::EthOnly(conn) => conn.into_inner(),
            Self::Satellite(conn) => conn.into_inner(),
            Self::EthSnapOnly(conn) => conn.into_inner(),
            Self::SnapSatellite(conn) => conn.into_inner(),
        }
    }

    /// Returns mutable access to the underlying stream.
    #[inline]
    pub(crate) fn inner_mut(&mut self) -> &mut P2PStream<ECIESStream<TcpStream>> {
        match self {
            Self::EthOnly(conn) => conn.inner_mut(),
            Self::Satellite(conn) => conn.inner_mut(),
            Self::EthSnapOnly(conn) => conn.inner_mut(),
            Self::SnapSatellite(conn) => conn.inner_mut(),
        }
    }

    /// Returns access to the underlying stream.
    #[inline]
    pub(crate) const fn inner(&self) -> &P2PStream<ECIESStream<TcpStream>> {
        match self {
            Self::EthOnly(conn) => conn.inner(),
            Self::Satellite(conn) => conn.inner(),
            Self::EthSnapOnly(conn) => conn.inner(),
            Self::SnapSatellite(conn) => conn.inner(),
        }
    }

    /// Same as [`Sink::start_send`] but accepts a [`EthBroadcastMessage`] instead.
    #[inline]
    pub fn start_send_broadcast(
        &mut self,
        item: EthBroadcastMessage<N>,
    ) -> Result<(), EthStreamError> {
        match self {
            Self::EthOnly(conn) => conn.start_send_broadcast(item),
            Self::Satellite(conn) => conn.primary_mut().start_send_broadcast(item),
            Self::EthSnapOnly(conn) => conn.start_send_broadcast(item).map_err(Into::into),
            Self::SnapSatellite(conn) => {
                conn.primary_mut().start_send_broadcast(item).map_err(Into::into)
            }
        }
    }

    /// Sends a raw capability message over the connection
    pub fn start_send_raw(&mut self, msg: RawCapabilityMessage) -> Result<(), EthStreamError> {
        match self {
            Self::EthOnly(conn) => conn.start_send_raw(msg),
            Self::Satellite(conn) => conn.primary_mut().start_send_raw(msg),
            Self::EthSnapOnly(conn) => conn.start_send_raw(msg).map_err(Into::into),
            Self::SnapSatellite(conn) => conn.primary_mut().start_send_raw(msg).map_err(Into::into),
        }
    }
}

impl<N: NetworkPrimitives> From<EthPeerConnection<N>> for EthRlpxConnection<N> {
    #[inline]
    fn from(conn: EthPeerConnection<N>) -> Self {
        Self::EthOnly(Box::new(conn))
    }
}

impl<N: NetworkPrimitives> From<EthSatelliteConnection<N>> for EthRlpxConnection<N> {
    #[inline]
    fn from(conn: EthSatelliteConnection<N>) -> Self {
        Self::Satellite(Box::new(conn))
    }
}

impl<N: NetworkPrimitives> From<EthSnapPeerConnection<N>> for EthRlpxConnection<N> {
    #[inline]
    fn from(conn: EthSnapPeerConnection<N>) -> Self {
        Self::EthSnapOnly(Box::new(conn))
    }
}

impl<N: NetworkPrimitives> From<EthSnapSatelliteConnection<N>> for EthRlpxConnection<N> {
    #[inline]
    fn from(conn: EthSnapSatelliteConnection<N>) -> Self {
        Self::SnapSatellite(Box::new(conn))
    }
}

impl<N: NetworkPrimitives> Stream for EthRlpxConnection<N> {
    type Item = Result<EthSnapMessage<N>, EthStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.project() {
            EthRlpxConnProj::EthOnly(conn) => {
                conn.poll_next(cx).map(|opt| opt.map(|res| res.map(EthSnapMessage::Eth)))
            }
            EthRlpxConnProj::Satellite(conn) => {
                conn.poll_next(cx).map(|opt| opt.map(|res| res.map(EthSnapMessage::Eth)))
            }
            EthRlpxConnProj::EthSnapOnly(conn) => {
                conn.poll_next(cx).map(|opt| opt.map(|res| res.map_err(Into::into)))
            }
            EthRlpxConnProj::SnapSatellite(conn) => {
                conn.poll_next(cx).map(|opt| opt.map(|res| res.map_err(Into::into)))
            }
        }
    }
}

impl<N: NetworkPrimitives> Sink<EthSnapMessage<N>> for EthRlpxConnection<N> {
    type Error = EthStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.project() {
            EthRlpxConnProj::EthOnly(conn) => conn.poll_ready(cx),
            EthRlpxConnProj::Satellite(conn) => conn.poll_ready(cx),
            EthRlpxConnProj::EthSnapOnly(conn) => conn.poll_ready(cx).map_err(Into::into),
            EthRlpxConnProj::SnapSatellite(conn) => conn.poll_ready(cx).map_err(Into::into),
        }
    }

    fn start_send(self: Pin<&mut Self>, item: EthSnapMessage<N>) -> Result<(), Self::Error> {
        match (self.project(), item) {
            (EthRlpxConnProj::EthOnly(mut conn), EthSnapMessage::Eth(msg)) => {
                conn.start_send_unpin(msg)
            }
            (EthRlpxConnProj::Satellite(mut conn), EthSnapMessage::Eth(msg)) => {
                conn.start_send_unpin(msg)
            }
            (EthRlpxConnProj::EthSnapOnly(mut conn), msg) => {
                conn.start_send_unpin(msg).map_err(Into::into)
            }
            (EthRlpxConnProj::SnapSatellite(mut conn), msg) => {
                conn.start_send_unpin(msg).map_err(Into::into)
            }
            // Sending any snap message over eth-only connection is unsupported
            (
                EthRlpxConnProj::EthOnly(_) | EthRlpxConnProj::Satellite(_),
                EthSnapMessage::Snap(msg),
            ) => Err(EthStreamError::UnsupportedMessage { message_id: msg.message_id() as u8 }),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.project() {
            EthRlpxConnProj::EthOnly(conn) => conn.poll_flush(cx),
            EthRlpxConnProj::Satellite(conn) => conn.poll_flush(cx),
            EthRlpxConnProj::EthSnapOnly(conn) => conn.poll_flush(cx).map_err(Into::into),
            EthRlpxConnProj::SnapSatellite(conn) => conn.poll_flush(cx).map_err(Into::into),
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.project() {
            EthRlpxConnProj::EthOnly(conn) => conn.poll_close(cx),
            EthRlpxConnProj::Satellite(conn) => conn.poll_close(cx),
            EthRlpxConnProj::EthSnapOnly(conn) => conn.poll_close(cx).map_err(Into::into),
            EthRlpxConnProj::SnapSatellite(conn) => conn.poll_close(cx).map_err(Into::into),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn assert_eth_snap_stream<N, St>()
    where
        N: NetworkPrimitives,
        St: Stream<Item = Result<EthSnapMessage<N>, EthStreamError>> + Sink<EthSnapMessage<N>>,
    {
    }

    #[test]
    const fn test_eth_stream_variants() {
        assert_eth_snap_stream::<EthNetworkPrimitives, EthRlpxConnection<EthNetworkPrimitives>>();
    }
}
