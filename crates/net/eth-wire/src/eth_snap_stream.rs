//! Ethereum ETH and SNAP combined protocol stream implementation.

use super::message::MAX_MESSAGE_SIZE;
use crate::{
    EthMessage, EthNetworkPrimitives, EthVersion, NetworkPrimitives, ProtocolMessage,
    SnapMessageId, SnapProtocolMessage,
};
use alloy_rlp::{Bytes, BytesMut, Encodable};
use core::fmt::Debug;
use futures::{Sink, SinkExt};
use pin_project::pin_project;
use std::{
    marker::PhantomData,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio_stream::Stream;

/// Error type for the ETH+SNAP stream
#[derive(thiserror::Error, Debug)]
pub enum EthSnapStreamError {
    /// Invalid message for protocol version
    #[error("invalid message for version {0:?}: {1}")]
    InvalidMessage(EthVersion, String),

    /// Unknown message ID
    #[error("unknown message id: {0}")]
    UnknownMessageId(u8),

    /// Message too large
    #[error("message too large: {0} > {1}")]
    MessageTooLarge(usize, usize),

    /// RLP decoding error
    #[error("rlp error: {0}")]
    Rlp(#[from] alloy_rlp::Error),

    /// Status message received outside handshake
    #[error("status message received outside handshake")]
    StatusNotInHandshake,
}

/// Combined message type that include either ETH or SNAP protocol messages
#[derive(Debug)]
pub enum EthSnapMessage<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// An Ethereum protocol message
    Eth(EthMessage<N>),
    /// A SNAP protocol message
    Snap(SnapProtocolMessage),
}

/// A stream implementation that can handle both ETH and SNAP protocol messages
/// over a single connection.
#[pin_project]
#[derive(Debug, Clone)]
pub struct EthSnapStream<S, N = EthNetworkPrimitives> {
    /// Protocol logic
    eth_snap: EthSnapStreamInner<N>,
    /// Inner byte stream
    #[pin]
    inner: S,
}

impl<S, N> EthSnapStream<S, N>
where
    N: NetworkPrimitives,
{
    /// Create a new eth and snap protocol stream
    pub fn new(stream: S, eth_version: EthVersion) -> Self {
        Self { eth_snap: EthSnapStreamInner::new(eth_version), inner: stream }
    }

    /// Returns the eth version
    #[inline]
    pub const fn eth_version(&self) -> EthVersion {
        self.eth_snap.eth_version()
    }

    /// Returns the underlying stream
    #[inline]
    pub const fn inner(&self) -> &S {
        &self.inner
    }

    /// Returns mutable access to the underlying stream
    #[inline]
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Consumes this type and returns the wrapped stream
    #[inline]
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S, E, N> Stream for EthSnapStream<S, N>
where
    S: Stream<Item = Result<BytesMut, E>> + Unpin,
    EthSnapStreamError: From<E>,
    N: NetworkPrimitives,
{
    type Item = Result<EthSnapMessage<N>, EthSnapStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let res = ready!(this.inner.poll_next(cx));

        match res {
            Some(Ok(bytes)) => Poll::Ready(Some(this.eth_snap.decode_message(bytes))),
            Some(Err(err)) => Poll::Ready(Some(Err(err.into()))),
            None => Poll::Ready(None),
        }
    }
}

impl<S, E, N> Sink<EthSnapMessage<N>> for EthSnapStream<S, N>
where
    S: Sink<Bytes, Error = E> + Unpin,
    EthSnapStreamError: From<E>,
    N: NetworkPrimitives,
{
    type Error = EthSnapStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_ready(cx).map_err(Into::into)
    }

    fn start_send(mut self: Pin<&mut Self>, item: EthSnapMessage<N>) -> Result<(), Self::Error> {
        let mut this = self.as_mut().project();

        let bytes = match item {
            EthSnapMessage::Eth(eth_msg) => this.eth_snap.encode_eth_message(eth_msg)?,
            EthSnapMessage::Snap(snap_msg) => this.eth_snap.encode_snap_message(snap_msg),
        };

        this.inner.start_send_unpin(bytes)?;
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_flush(cx).map_err(Into::into)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_close(cx).map_err(Into::into)
    }
}

/// Stream handling combined eth and snap protocol logic
/// Snap version is not critical to specify yet,
/// Only one version, snap/1, does exist.
#[derive(Debug, Clone)]
struct EthSnapStreamInner<N> {
    /// ETH protocol version
    eth_version: EthVersion,
    /// Type marker
    _pd: PhantomData<N>,
}

impl<N> EthSnapStreamInner<N>
where
    N: NetworkPrimitives,
{
    /// Create a new eth and snap protocol stream
    const fn new(eth_version: EthVersion) -> Self {
        Self { eth_version, _pd: PhantomData }
    }

    #[inline]
    const fn eth_version(&self) -> EthVersion {
        self.eth_version
    }

    /// Decode a message from the stream
    fn decode_message(&self, bytes: BytesMut) -> Result<EthSnapMessage<N>, EthSnapStreamError> {
        if bytes.len() > MAX_MESSAGE_SIZE {
            return Err(EthSnapStreamError::MessageTooLarge(bytes.len(), MAX_MESSAGE_SIZE));
        }

        if bytes.is_empty() {
            return Err(EthSnapStreamError::Rlp(alloy_rlp::Error::InputTooShort));
        }

        let message_id = bytes[0];

        if message_id <= crate::message::EthMessageID::max() {
            let mut buf = bytes.as_ref();
            match ProtocolMessage::decode_message(self.eth_version, &mut buf) {
                Ok(protocol_msg) => {
                    if matches!(protocol_msg.message, EthMessage::Status(_)) {
                        return Err(EthSnapStreamError::StatusNotInHandshake);
                    }
                    Ok(EthSnapMessage::Eth(protocol_msg.message))
                }
                Err(err) => {
                    Err(EthSnapStreamError::InvalidMessage(self.eth_version, err.to_string()))
                }
            }
        } else if message_id <= SnapMessageId::TrieNodes as u8 {
            let mut buf = &bytes[1..];

            match SnapProtocolMessage::decode(message_id, &mut buf) {
                Ok(snap_msg) => Ok(EthSnapMessage::Snap(snap_msg)),
                Err(err) => Err(EthSnapStreamError::Rlp(err)),
            }
        } else {
            Err(EthSnapStreamError::UnknownMessageId(message_id))
        }
    }

    /// Encode an eth message
    fn encode_eth_message(&self, item: EthMessage<N>) -> Result<Bytes, EthSnapStreamError> {
        if matches!(item, EthMessage::Status(_)) {
            return Err(EthSnapStreamError::StatusNotInHandshake);
        }

        let protocol_msg = ProtocolMessage::from(item);
        let mut buf = Vec::new();
        protocol_msg.encode(&mut buf);
        Ok(Bytes::from(buf))
    }

    /// Encode an snap message
    fn encode_snap_message(&self, message: SnapProtocolMessage) -> Bytes {
        message.encode().into()
    }
}
