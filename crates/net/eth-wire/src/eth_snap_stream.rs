//! Ethereum and snap combined protocol stream implementation.
//!
//! A stream type for handling both eth and snap protocol messages over a single `RLPx` connection.
//! Provides message encoding/decoding, ID multiplexing, and protocol message processing.

use super::message::MAX_MESSAGE_SIZE;
use crate::{
    message::{EthBroadcastMessage, ProtocolBroadcastMessage},
    EthMessage, EthMessageID, EthNetworkPrimitives, EthVersion, NetworkPrimitives, ProtocolMessage,
    RawCapabilityMessage, SnapMessageId, SnapProtocolMessage,
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

/// Error type for the eth and snap stream
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

/// Combined message type that include either eth or snao protocol messages
#[derive(Debug)]
pub enum EthSnapMessage<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// An Ethereum protocol message
    Eth(EthMessage<N>),
    /// A snap protocol message
    Snap(SnapProtocolMessage),
}

/// A stream implementation that can handle both eth and snap protocol messages
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
    pub const fn new(stream: S, eth_version: EthVersion) -> Self {
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
    pub const fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Consumes this type and returns the wrapped stream
    #[inline]
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S, E, N> EthSnapStream<S, N>
where
    S: Sink<Bytes, Error = E> + Unpin,
    EthSnapStreamError: From<E>,
    N: NetworkPrimitives,
{
    /// Same as [`Sink::start_send`] but accepts a [`EthBroadcastMessage`] instead.
    pub fn start_send_broadcast(
        &mut self,
        item: EthBroadcastMessage<N>,
    ) -> Result<(), EthSnapStreamError> {
        self.inner.start_send_unpin(Bytes::from(alloy_rlp::encode(
            ProtocolBroadcastMessage::from(item),
        )))?;

        Ok(())
    }

    /// Sends a raw capability message directly over the stream
    pub fn start_send_raw(&mut self, msg: RawCapabilityMessage) -> Result<(), EthSnapStreamError> {
        let mut bytes = Vec::with_capacity(msg.payload.len() + 1);
        msg.id.encode(&mut bytes);
        bytes.extend_from_slice(&msg.payload);

        self.inner.start_send_unpin(bytes.into())?;
        Ok(())
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
    /// Eth protocol version
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

        // This check works because capabilities are sorted lexicographically
        // if "eth" before "snap", giving eth messages lower IDs than snap messages,
        // and eth message IDs are <= [`EthMessageID::max()`],
        // snap message IDs are > [`EthMessageID::max()`].
        // See also <https://github.com/paradigmxyz/reth/blob/main/crates/net/eth-wire/src/capability.rs#L272-L283>.
        if message_id <= EthMessageID::max() {
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
        } else if message_id > EthMessageID::max() &&
            message_id <= EthMessageID::max() + 1 + SnapMessageId::TrieNodes as u8
        {
            // Checks for multiplexed snap message IDs :
            // - message_id > EthMessageID::max() : ensures it's not an eth message
            // - message_id <= EthMessageID::max() + 1 + snap_max : ensures it's within valid snap
            //   range
            // Message IDs are assigned lexicographically during capability negotiation
            // So real_snap_id = multiplexed_id - num_eth_messages
            let adjusted_message_id = message_id - (EthMessageID::max() + 1);
            let mut buf = &bytes[1..];

            match SnapProtocolMessage::decode(adjusted_message_id, &mut buf) {
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

    /// Encode a snap protocol message, adjusting the message ID to follow eth message IDs
    /// for proper multiplexing.
    fn encode_snap_message(&self, message: SnapProtocolMessage) -> Bytes {
        let encoded = message.encode();

        let message_id = encoded[0];
        let adjusted_id = message_id + EthMessageID::max() + 1;

        let mut adjusted = Vec::with_capacity(encoded.len());
        adjusted.push(adjusted_id);
        adjusted.extend_from_slice(&encoded[1..]);

        Bytes::from(adjusted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EthMessage, SnapProtocolMessage};
    use alloy_eips::BlockHashOrNumber;
    use alloy_primitives::B256;
    use alloy_rlp::Encodable;
    use reth_eth_wire_types::{
        message::RequestPair, GetAccountRangeMessage, GetBlockHeaders, HeadersDirection,
    };

    // Helper to create eth message and its bytes
    fn create_eth_message() -> (EthMessage<EthNetworkPrimitives>, BytesMut) {
        let eth_msg = EthMessage::<EthNetworkPrimitives>::GetBlockHeaders(RequestPair {
            request_id: 1,
            message: GetBlockHeaders {
                start_block: BlockHashOrNumber::Number(1),
                limit: 10,
                skip: 0,
                direction: HeadersDirection::Rising,
            },
        });

        let protocol_msg = ProtocolMessage::from(eth_msg.clone());
        let mut buf = Vec::new();
        protocol_msg.encode(&mut buf);

        (eth_msg, BytesMut::from(&buf[..]))
    }

    // Helper to create snap message and its bytes
    fn create_snap_message() -> (SnapProtocolMessage, BytesMut) {
        let snap_msg = SnapProtocolMessage::GetAccountRange(GetAccountRangeMessage {
            request_id: 1,
            root_hash: B256::default(),
            starting_hash: B256::default(),
            limit_hash: B256::default(),
            response_bytes: 1000,
        });

        let inner = EthSnapStreamInner::<EthNetworkPrimitives>::new(EthVersion::Eth67);
        let encoded = inner.encode_snap_message(snap_msg.clone());

        (snap_msg, BytesMut::from(&encoded[..]))
    }

    #[test]
    fn test_eth_message_roundtrip() {
        let inner = EthSnapStreamInner::<EthNetworkPrimitives>::new(EthVersion::Eth67);
        let (eth_msg, eth_bytes) = create_eth_message();

        // Verify encoding
        let encoded_result = inner.encode_eth_message(eth_msg.clone());
        assert!(encoded_result.is_ok());

        // Verify decoding
        let decoded_result = inner.decode_message(eth_bytes.clone());
        assert!(matches!(decoded_result, Ok(EthSnapMessage::Eth(_))));

        // round trip
        if let Ok(EthSnapMessage::Eth(decoded_msg)) = inner.decode_message(eth_bytes) {
            assert_eq!(decoded_msg, eth_msg);

            let re_encoded = inner.encode_eth_message(decoded_msg.clone()).unwrap();
            let re_encoded_bytes = BytesMut::from(&re_encoded[..]);
            let re_decoded = inner.decode_message(re_encoded_bytes);

            assert!(matches!(re_decoded, Ok(EthSnapMessage::Eth(_))));
            if let Ok(EthSnapMessage::Eth(final_msg)) = re_decoded {
                assert_eq!(final_msg, decoded_msg);
            }
        }
    }

    #[test]
    fn test_snap_protocol() {
        let inner = EthSnapStreamInner::<EthNetworkPrimitives>::new(EthVersion::Eth67);
        let (snap_msg, snap_bytes) = create_snap_message();

        // Verify encoding
        let encoded_bytes = inner.encode_snap_message(snap_msg.clone());
        assert!(!encoded_bytes.is_empty());

        // Verify decoding
        let decoded_result = inner.decode_message(snap_bytes.clone());
        assert!(matches!(decoded_result, Ok(EthSnapMessage::Snap(_))));

        // round trip
        if let Ok(EthSnapMessage::Snap(decoded_msg)) = inner.decode_message(snap_bytes) {
            assert_eq!(decoded_msg, snap_msg);

            // re-encode message
            let encoded = inner.encode_snap_message(decoded_msg.clone());

            let re_encoded_bytes = BytesMut::from(&encoded[..]);

            // decode with properly adjusted ID
            let re_decoded = inner.decode_message(re_encoded_bytes);

            assert!(matches!(re_decoded, Ok(EthSnapMessage::Snap(_))));
            if let Ok(EthSnapMessage::Snap(final_msg)) = re_decoded {
                assert_eq!(final_msg, decoded_msg);
            }
        }
    }

    #[test]
    fn test_message_id_boundaries() {
        let inner = EthSnapStreamInner::<EthNetworkPrimitives>::new(EthVersion::Eth67);

        // Create a bytes buffer with eth message ID at the max boundary with minimal content
        let eth_max_id = EthMessageID::max();
        let mut eth_boundary_bytes = BytesMut::new();
        eth_boundary_bytes.extend_from_slice(&[eth_max_id]);
        eth_boundary_bytes.extend_from_slice(&[0, 0]);

        // This should be decoded as eth message
        let eth_boundary_result = inner.decode_message(eth_boundary_bytes);
        assert!(
            eth_boundary_result.is_err() ||
                matches!(eth_boundary_result, Ok(EthSnapMessage::Eth(_)))
        );

        // Create a bytes buffer with message ID just above eth max, it should be snap min
        let snap_min_id = eth_max_id + 1;
        let mut snap_boundary_bytes = BytesMut::new();
        snap_boundary_bytes.extend_from_slice(&[snap_min_id]);
        snap_boundary_bytes.extend_from_slice(&[0, 0]);

        // Not a valid snap message yet, only snap id --> error
        let snap_boundary_result = inner.decode_message(snap_boundary_bytes);
        assert!(snap_boundary_result.is_err());
    }
}
