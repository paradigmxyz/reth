//! Connection types for a session

use futures::{Sink, SinkExt, Stream, StreamExt};
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    errors::EthStreamError,
    message::EthBroadcastMessage,
    multiplex::{ProtocolProxy, RlpxSatelliteStream},
    EthMessage, EthNetworkPrimitives, EthSnapMessage, EthSnapStream, EthStream, EthVersion,
    NetworkPrimitives, P2PStream,
};
use reth_eth_wire_types::RawCapabilityMessage;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::net::TcpStream;

/// The type of the underlying peer network connection.
pub type EthPeerConnection<N> = EthStream<P2PStream<ECIESStream<TcpStream>>, N>;

/// Various connection types that at least support the ETH protocol.
pub type EthSatelliteConnection<N = EthNetworkPrimitives> =
    RlpxSatelliteStream<ECIESStream<TcpStream>, EthStream<ProtocolProxy, N>>;

/// A dedicated `eth` + `snap/2` connection.
pub type EthSnapConnection<N = EthNetworkPrimitives> = EthSnapStream<ECIESStream<TcpStream>, N>;

/// Connection types that support the ETH protocol.
///
/// This can be either:
/// - A connection that only supports the ETH protocol
/// - A connection that supports the ETH protocol and `snap/2` ([`EthSnapStream`])
/// - A connection that supports the ETH protocol and at least one other `RLPx` protocol
// This type is boxed because the underlying stream is ~6KB,
// mostly coming from `P2PStream`'s `snap::Encoder` (2072), and `ECIESStream` (3600).
#[derive(Debug)]
pub enum EthRlpxConnection<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// A connection that only supports the ETH protocol.
    EthOnly(Box<EthPeerConnection<N>>),
    /// A dedicated connection that supports the ETH protocol and `snap/2` (EIP-8189).
    EthSnap(Box<EthSnapConnection<N>>),
    /// A connection that supports the ETH protocol and __at least one other__ `RLPx` protocol.
    Satellite(Box<EthSatelliteConnection<N>>),
}

impl<N: NetworkPrimitives> EthRlpxConnection<N> {
    /// Returns the negotiated ETH version.
    #[inline]
    pub(crate) const fn version(&self) -> EthVersion {
        match self {
            Self::EthOnly(conn) => conn.version(),
            Self::EthSnap(conn) => conn.version(),
            Self::Satellite(conn) => conn.primary().version(),
        }
    }

    /// Consumes this type and returns the wrapped [`P2PStream`].
    #[inline]
    pub(crate) fn into_inner(self) -> P2PStream<ECIESStream<TcpStream>> {
        match self {
            Self::EthOnly(conn) => conn.into_inner(),
            Self::EthSnap(conn) => conn.into_inner(),
            Self::Satellite(conn) => conn.into_inner(),
        }
    }

    /// Returns mutable access to the underlying stream.
    #[inline]
    pub(crate) fn inner_mut(&mut self) -> &mut P2PStream<ECIESStream<TcpStream>> {
        match self {
            Self::EthOnly(conn) => conn.inner_mut(),
            Self::EthSnap(conn) => conn.inner_mut(),
            Self::Satellite(conn) => conn.inner_mut(),
        }
    }

    /// Returns access to the underlying stream.
    #[inline]
    pub(crate) const fn inner(&self) -> &P2PStream<ECIESStream<TcpStream>> {
        match self {
            Self::EthOnly(conn) => conn.inner(),
            Self::EthSnap(conn) => conn.inner(),
            Self::Satellite(conn) => conn.inner(),
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
            Self::EthSnap(conn) => conn.start_send_broadcast(item),
            Self::Satellite(conn) => conn.primary_mut().start_send_broadcast(item),
        }
    }

    /// Sends a raw capability message over the connection
    pub fn start_send_raw(&mut self, msg: RawCapabilityMessage) -> Result<(), EthStreamError> {
        match self {
            Self::EthOnly(conn) => conn.start_send_raw(msg),
            Self::EthSnap(conn) => conn.start_send_raw(msg),
            Self::Satellite(conn) => conn.primary_mut().start_send_raw(msg),
        }
    }

    /// Sets whether to reject block announcement messages (`NewBlock`, `NewBlockHashes`) before
    /// RLP decoding to avoid memory amplification from deserializing blocks that will be discarded.
    pub fn set_reject_block_announcements(&mut self, reject: bool) {
        match self {
            Self::EthOnly(conn) => conn.set_reject_block_announcements(reject),
            Self::EthSnap(conn) => conn.set_reject_block_announcements(reject),
            Self::Satellite(conn) => conn.primary_mut().set_reject_block_announcements(reject),
        }
    }
}

impl<N: NetworkPrimitives> From<EthPeerConnection<N>> for EthRlpxConnection<N> {
    #[inline]
    fn from(conn: EthPeerConnection<N>) -> Self {
        Self::EthOnly(Box::new(conn))
    }
}

impl<N: NetworkPrimitives> From<EthSnapConnection<N>> for EthRlpxConnection<N> {
    #[inline]
    fn from(conn: EthSnapConnection<N>) -> Self {
        Self::EthSnap(Box::new(conn))
    }
}

impl<N: NetworkPrimitives> From<EthSatelliteConnection<N>> for EthRlpxConnection<N> {
    #[inline]
    fn from(conn: EthSatelliteConnection<N>) -> Self {
        Self::Satellite(Box::new(conn))
    }
}

/// Delegates a call to the active variant's boxed stream (every variant is `Unpin`).
///
/// The second form runs `$adapt` on the eth-only variants to lift their result into the shared
/// item type; the snap variant already yields it.
macro_rules! delegate_call {
    ($self:ident.$method:ident($($args:ident),+)) => {
        match $self.get_mut() {
            Self::EthOnly(l) => l.$method($($args),+),
            Self::EthSnap(s) => s.$method($($args),+),
            Self::Satellite(r) => r.$method($($args),+),
        }
    };
    ($self:ident.$method:ident($($args:ident),+) => $adapt:expr) => {
        match $self.get_mut() {
            Self::EthOnly(l) => $adapt(l.$method($($args),+)),
            Self::Satellite(r) => $adapt(r.$method($($args),+)),
            Self::EthSnap(s) => s.$method($($args),+),
        }
    };
}

impl<N: NetworkPrimitives> Stream for EthRlpxConnection<N> {
    type Item = Result<EthSnapMessage<N>, EthStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        delegate_call!(self.poll_next_unpin(cx) => lift_eth)
    }
}

impl<N: NetworkPrimitives> Sink<EthMessage<N>> for EthRlpxConnection<N> {
    type Error = EthStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        delegate_call!(self.poll_ready_unpin(cx))
    }

    fn start_send(self: Pin<&mut Self>, item: EthMessage<N>) -> Result<(), Self::Error> {
        match self.get_mut() {
            Self::EthOnly(l) => l.start_send_unpin(item),
            Self::Satellite(r) => r.start_send_unpin(item),
            Self::EthSnap(s) => s.start_send_unpin(EthSnapMessage::Eth(item)),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        delegate_call!(self.poll_flush_unpin(cx))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        delegate_call!(self.poll_close_unpin(cx))
    }
}

/// Lifts a polled `eth` item into the shared [`EthSnapMessage`] item type.
#[inline]
fn lift_eth<N: NetworkPrimitives>(
    poll: Poll<Option<Result<EthMessage<N>, EthStreamError>>>,
) -> Poll<Option<Result<EthSnapMessage<N>, EthStreamError>>> {
    poll.map(|opt| opt.map(|res| res.map(EthSnapMessage::Eth)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn assert_eth_stream<N, St>()
    where
        N: NetworkPrimitives,
        St: Stream<Item = Result<EthMessage<N>, EthStreamError>> + Sink<EthMessage<N>>,
    {
    }

    const fn assert_eth_snap_stream<N, St>()
    where
        N: NetworkPrimitives,
        St: Stream<Item = Result<EthSnapMessage<N>, EthStreamError>> + Sink<EthMessage<N>>,
    {
    }

    #[test]
    const fn test_eth_stream_variants() {
        assert_eth_stream::<EthNetworkPrimitives, EthSatelliteConnection<EthNetworkPrimitives>>();
        assert_eth_snap_stream::<EthNetworkPrimitives, EthRlpxConnection<EthNetworkPrimitives>>();
    }
}
