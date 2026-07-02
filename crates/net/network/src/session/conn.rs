//! Connection types for a session

use alloy_primitives::bytes::BytesMut;
use futures::{Sink, SinkExt, Stream};
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    errors::EthStreamError,
    message::EthBroadcastMessage,
    multiplex::{ProtocolProxy, RlpxSatelliteStream},
    EthMessage, EthNetworkPrimitives, EthStream, EthVersion, NetworkPrimitives, P2PStream,
    SharedTransactions,
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

/// Connection types that support the ETH protocol.
///
/// This can be either:
/// - A connection that only supports the ETH protocol
/// - A connection that supports the ETH protocol and at least one other `RLPx` protocol
// This type is boxed because the underlying stream is ~6KB,
// mostly coming from `P2PStream`'s `snap::Encoder` (2072), and `ECIESStream` (3600).
#[derive(Debug)]
pub enum EthRlpxConnection<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// A connection that only supports the ETH protocol.
    EthOnly(Box<EthPeerConnection<N>>),
    /// A connection that supports the ETH protocol and __at least one other__ `RLPx` protocol.
    Satellite(Box<EthSatelliteConnection<N>>),
}

impl<N: NetworkPrimitives> EthRlpxConnection<N> {
    /// Returns the negotiated ETH version.
    #[inline]
    pub(crate) const fn version(&self) -> EthVersion {
        match self {
            Self::EthOnly(conn) => conn.version(),
            Self::Satellite(conn) => conn.primary().version(),
        }
    }

    /// Consumes this type and returns the wrapped [`P2PStream`].
    #[inline]
    pub(crate) fn into_inner(self) -> P2PStream<ECIESStream<TcpStream>> {
        match self {
            Self::EthOnly(conn) => conn.into_inner(),
            Self::Satellite(conn) => conn.into_inner(),
        }
    }

    /// Returns mutable access to the underlying stream.
    #[inline]
    pub(crate) fn inner_mut(&mut self) -> &mut P2PStream<ECIESStream<TcpStream>> {
        match self {
            Self::EthOnly(conn) => conn.inner_mut(),
            Self::Satellite(conn) => conn.inner_mut(),
        }
    }

    /// Returns access to the underlying stream.
    #[inline]
    pub(crate) const fn inner(&self) -> &P2PStream<ECIESStream<TcpStream>> {
        match self {
            Self::EthOnly(conn) => conn.inner(),
            Self::Satellite(conn) => conn.inner(),
        }
    }

    /// Returns how many messages can be started after one successful readiness poll.
    #[inline]
    pub(crate) fn available_outgoing_capacity(&self) -> usize {
        match self {
            Self::EthOnly(conn) => conn.inner().available_outgoing_capacity(),
            // Satellite eth messages are first queued into the protocol proxy. Keep the generic
            // sink contract for that path and only batch direct eth-only p2p streams.
            Self::Satellite(_) => 1,
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
        }
    }

    /// Sends an eth message using caller-owned scratch space for eth-only connections.
    #[inline]
    pub fn start_send_with_encode_buf(
        &mut self,
        item: EthMessage<N>,
        encode_buf: &mut BytesMut,
    ) -> Result<(), EthStreamError> {
        match self {
            Self::EthOnly(conn) => conn.start_send_with_encode_buf(item, encode_buf),
            Self::Satellite(conn) => conn.primary_mut().start_send_unpin(item),
        }
    }

    /// Sends an eth broadcast using caller-owned scratch space for eth-only connections.
    #[inline]
    pub fn start_send_broadcast_with_encode_buf(
        &mut self,
        item: EthBroadcastMessage<N>,
        encode_buf: &mut BytesMut,
    ) -> Result<(), EthStreamError> {
        match self {
            Self::EthOnly(conn) => conn.start_send_broadcast_with_encode_buf(item, encode_buf),
            Self::Satellite(conn) => conn.primary_mut().start_send_broadcast(item),
        }
    }

    /// Sends a transactions broadcast with a precomputed RLP payload length.
    #[inline]
    pub fn start_send_transactions_with_payload_length(
        &mut self,
        transactions: SharedTransactions<N::BroadcastedTransaction>,
        payload_length: usize,
    ) -> Result<(), EthStreamError> {
        match self {
            Self::EthOnly(conn) => {
                conn.start_send_transactions_with_payload_length(transactions, payload_length)
            }
            Self::Satellite(conn) => conn
                .primary_mut()
                .start_send_transactions_with_payload_length(transactions, payload_length),
        }
    }

    /// Sends a transactions broadcast using caller-owned scratch space for eth-only connections.
    #[inline]
    pub fn start_send_transactions_with_payload_length_and_encode_buf(
        &mut self,
        transactions: SharedTransactions<N::BroadcastedTransaction>,
        payload_length: usize,
        encode_buf: &mut BytesMut,
    ) -> Result<(), EthStreamError> {
        match self {
            Self::EthOnly(conn) => conn.start_send_transactions_with_payload_length_and_encode_buf(
                transactions,
                payload_length,
                encode_buf,
            ),
            Self::Satellite(conn) => conn
                .primary_mut()
                .start_send_transactions_with_payload_length(transactions, payload_length),
        }
    }

    /// Sends a raw capability message over the connection
    pub fn start_send_raw(&mut self, msg: RawCapabilityMessage) -> Result<(), EthStreamError> {
        match self {
            Self::EthOnly(conn) => conn.start_send_raw(msg),
            Self::Satellite(conn) => conn.primary_mut().start_send_raw(msg),
        }
    }

    /// Sends a raw capability message using caller-owned scratch space for eth-only connections.
    pub fn start_send_raw_with_encode_buf(
        &mut self,
        msg: RawCapabilityMessage,
        encode_buf: &mut BytesMut,
    ) -> Result<(), EthStreamError> {
        match self {
            Self::EthOnly(conn) => conn.start_send_raw_with_encode_buf(msg, encode_buf),
            Self::Satellite(conn) => conn.primary_mut().start_send_raw(msg),
        }
    }

    /// Sets whether to reject block announcement messages (`NewBlock`, `NewBlockHashes`) before
    /// RLP decoding to avoid memory amplification from deserializing blocks that will be discarded.
    pub fn set_reject_block_announcements(&mut self, reject: bool) {
        match self {
            Self::EthOnly(conn) => conn.set_reject_block_announcements(reject),
            Self::Satellite(conn) => conn.primary_mut().set_reject_block_announcements(reject),
        }
    }

    /// Polls the next eth message, using `decode_buf` as scratch space for eth-only p2p streams.
    pub(crate) fn poll_next_eth_message(
        &mut self,
        cx: &mut Context<'_>,
        decode_buf: &mut BytesMut,
    ) -> Poll<Option<Result<EthMessage<N>, EthStreamError>>> {
        unsafe {
            match self {
                Self::EthOnly(conn) => {
                    Pin::new_unchecked(&mut **conn).poll_next_eth_message(cx, decode_buf)
                }
                Self::Satellite(conn) => Pin::new_unchecked(conn).poll_next(cx),
            }
        }
    }

    /// Polls send readiness, bypassing the generic eth stream sink wrapper for eth-only sessions.
    #[inline]
    pub(crate) fn poll_ready_session(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), EthStreamError>> {
        unsafe {
            match self {
                Self::EthOnly(conn) => Pin::new_unchecked(conn.inner_mut())
                    .poll_ready_ecies_buffered(cx)
                    .map_err(Into::into),
                Self::Satellite(conn) => Pin::new_unchecked(conn).poll_ready(cx),
            }
        }
    }

    /// Flushes the connection, using the ECIES-aware buffered drain path for eth-only sessions.
    pub(crate) fn poll_flush_buffered(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), EthStreamError>> {
        unsafe {
            match self {
                Self::EthOnly(conn) => Pin::new_unchecked(conn.inner_mut())
                    .poll_flush_ecies_buffered(cx)
                    .map_err(Into::into),
                Self::Satellite(conn) => Pin::new_unchecked(conn).poll_flush(cx),
            }
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

macro_rules! delegate_call {
    ($self:ident.$method:ident($($args:ident),+)) => {
        unsafe {
            match $self.get_unchecked_mut() {
                Self::EthOnly(l) => Pin::new_unchecked(l).$method($($args),+),
                Self::Satellite(r) => Pin::new_unchecked(r).$method($($args),+),
            }
        }
    }
}

impl<N: NetworkPrimitives> Stream for EthRlpxConnection<N> {
    type Item = Result<EthMessage<N>, EthStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        delegate_call!(self.poll_next(cx))
    }
}

impl<N: NetworkPrimitives> Sink<EthMessage<N>> for EthRlpxConnection<N> {
    type Error = EthStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        delegate_call!(self.poll_ready(cx))
    }

    fn start_send(self: Pin<&mut Self>, item: EthMessage<N>) -> Result<(), Self::Error> {
        delegate_call!(self.start_send(item))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        delegate_call!(self.poll_flush(cx))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        delegate_call!(self.poll_close(cx))
    }
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

    #[test]
    const fn test_eth_stream_variants() {
        assert_eth_stream::<EthNetworkPrimitives, EthSatelliteConnection<EthNetworkPrimitives>>();
        assert_eth_stream::<EthNetworkPrimitives, EthRlpxConnection<EthNetworkPrimitives>>();
    }
}
