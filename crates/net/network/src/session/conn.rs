//! Connection types for a session

use futures::{Sink, Stream};
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    errors::EthStreamError,
    message::EthBroadcastMessage,
    multiplex::{ProtocolProxy, RlpxSatelliteStream},
    EthMessage, EthStream, EthVersion, P2PStream,
};
use reth_net_common::bandwidth_meter::MeteredStream;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::net::TcpStream;

/// The type of the underlying peer network connection.
pub type EthPeerConnection = EthStream<P2PStream<ECIESStream<MeteredStream<TcpStream>>>>;

/// Various connection types that at least support the ETH protocol.
pub type EthSatelliteConnection =
    RlpxSatelliteStream<ECIESStream<MeteredStream<TcpStream>>, EthStream<ProtocolProxy>>;

/// Connection types that support the ETH protocol.
///
/// Either a [`EthPeerConnection`] or an [`EthSatelliteConnection`].
// This type is boxed because the underlying stream is ~6KB,
// mostly coming from `P2PStream`'s `snap::Encoder` (2072), and `ECIESStream` (3600).
#[derive(Debug)]
pub enum EthRlpxConnection {
    /// A That only supports the ETH protocol.
    EthOnly(Box<EthPeerConnection>),
    /// A connection that supports the ETH protocol and __at least one other__ RLPx protocol.
    Satellite(Box<EthSatelliteConnection>),
}

impl EthRlpxConnection {
    /// Returns the negotiated ETH version.
    #[inline]
    pub(crate) fn version(&self) -> EthVersion {
        match self {
            Self::EthOnly(conn) => conn.version(),
            Self::Satellite(conn) => conn.primary().version(),
        }
    }

    /// Consumes this type and returns the wrapped [P2PStream].
    #[inline]
    pub(crate) fn into_inner(self) -> P2PStream<ECIESStream<MeteredStream<TcpStream>>> {
        match self {
            Self::EthOnly(conn) => conn.into_inner(),
            Self::Satellite(conn) => conn.into_inner(),
        }
    }

    /// Returns mutable access to the underlying stream.
    #[inline]
    pub(crate) fn inner_mut(&mut self) -> &mut P2PStream<ECIESStream<MeteredStream<TcpStream>>> {
        match self {
            Self::EthOnly(conn) => conn.inner_mut(),
            Self::Satellite(conn) => conn.inner_mut(),
        }
    }

    /// Returns  access to the underlying stream.
    #[inline]
    pub(crate) fn inner(&self) -> &P2PStream<ECIESStream<MeteredStream<TcpStream>>> {
        match self {
            Self::EthOnly(conn) => conn.inner(),
            Self::Satellite(conn) => conn.inner(),
        }
    }

    /// Same as [`Sink::start_send`] but accepts a [`EthBroadcastMessage`] instead.
    #[inline]
    pub fn start_send_broadcast(
        &mut self,
        item: EthBroadcastMessage,
    ) -> Result<(), EthStreamError> {
        match self {
            Self::EthOnly(conn) => conn.start_send_broadcast(item),
            Self::Satellite(conn) => conn.primary_mut().start_send_broadcast(item),
        }
    }
}

impl From<EthPeerConnection> for EthRlpxConnection {
    #[inline]
    fn from(conn: EthPeerConnection) -> Self {
        Self::EthOnly(Box::new(conn))
    }
}

impl From<EthSatelliteConnection> for EthRlpxConnection {
    #[inline]
    fn from(conn: EthSatelliteConnection) -> Self {
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

impl Stream for EthRlpxConnection {
    type Item = Result<EthMessage, EthStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        delegate_call!(self.poll_next(cx))
    }
}

impl Sink<EthMessage> for EthRlpxConnection {
    type Error = EthStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        delegate_call!(self.poll_ready(cx))
    }

    fn start_send(self: Pin<&mut Self>, item: EthMessage) -> Result<(), Self::Error> {
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

    fn assert_eth_stream<St>()
    where
        St: Stream<Item = Result<EthMessage, EthStreamError>> + Sink<EthMessage>,
    {
    }

    #[test]
    fn test_eth_stream_variants() {
        assert_eth_stream::<EthSatelliteConnection>();
        assert_eth_stream::<EthRlpxConnection>();
    }
}
