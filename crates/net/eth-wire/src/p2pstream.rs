#![allow(dead_code, unreachable_pub, missing_docs, unused_variables)]
use crate::{
    capability::{Capability, SharedCapability},
    error::{P2PHandshakeError, P2PStreamError},
    pinger::{Pinger, PingerEvent},
    DisconnectReason, HelloMessage,
};
use bytes::{Buf, Bytes, BytesMut};
use futures::{Sink, SinkExt, StreamExt};
use metrics::counter;
use pin_project::pin_project;
use reth_rlp::{Decodable, DecodeError, Encodable, EMPTY_STRING_CODE};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    io,
    pin::Pin,
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio_stream::Stream;

/// [`MAX_PAYLOAD_SIZE`] is the maximum size of an uncompressed message payload.
/// This is defined in [EIP-706](https://eips.ethereum.org/EIPS/eip-706).
const MAX_PAYLOAD_SIZE: usize = 16 * 1024 * 1024;

/// [`MAX_RESERVED_MESSAGE_ID`] is the maximum message ID reserved for the `p2p` subprotocol. If
/// there are any incoming messages with an ID greater than this, they are subprotocol messages.
const MAX_RESERVED_MESSAGE_ID: u8 = 0x0f;

/// [`MAX_P2P_MESSAGE_ID`] is the maximum message ID in use for the `p2p` subprotocol.
const MAX_P2P_MESSAGE_ID: u8 = P2PMessageID::Pong as u8;

/// [`HANDSHAKE_TIMEOUT`] determines the amount of time to wait before determining that a `p2p`
/// handshake has timed out.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// [`PING_TIMEOUT`] determines the amount of time to wait before determining that a `p2p` ping has
/// timed out.
const PING_TIMEOUT: Duration = Duration::from_secs(15);

/// [`PING_INTERVAL`] determines the amount of time to wait between sending `p2p` ping messages
/// when the peer is responsive.
const PING_INTERVAL: Duration = Duration::from_secs(60);

/// [`GRACE_PERIOD`] determines the amount of time to wait for a peer to disconnect after sending a
/// [`P2PMessage::Disconnect`] message.
const GRACE_PERIOD: Duration = Duration::from_secs(2);

/// [`MAX_P2P_CAPACITY`] is the maximum number of messages that can be buffered to be sent in the
/// `p2p` stream.
const MAX_P2P_CAPACITY: usize = 64;

/// An un-authenticated [`P2PStream`]. This is consumed and returns a [`P2PStream`] after the
/// `Hello` handshake is completed.
#[pin_project]
pub struct UnauthedP2PStream<S> {
    #[pin]
    inner: S,
}

impl<S> UnauthedP2PStream<S> {
    /// Create a new `UnauthedP2PStream` from a type `S` which implements `Stream` and `Sink`.
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S> UnauthedP2PStream<S>
where
    S: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
{
    /// Consumes the `UnauthedP2PStream` and returns a `P2PStream` after the `Hello` handshake is
    /// completed successfully. This also returns the `Hello` message sent by the remote peer.
    pub async fn handshake(
        mut self,
        hello: HelloMessage,
    ) -> Result<(P2PStream<S>, HelloMessage), P2PStreamError> {
        tracing::trace!("sending p2p hello ...");

        // send our hello message with the Sink
        let mut raw_hello_bytes = BytesMut::new();
        P2PMessage::Hello(hello.clone()).encode(&mut raw_hello_bytes);
        self.inner.send(raw_hello_bytes.into()).await?;

        tracing::trace!("waiting for p2p hello from peer ...");

        let first_message_bytes = tokio::time::timeout(HANDSHAKE_TIMEOUT, self.inner.next())
            .await
            .or(Err(P2PStreamError::HandshakeError(P2PHandshakeError::Timeout)))?
            .ok_or(P2PStreamError::HandshakeError(P2PHandshakeError::NoResponse))??;

        // let's check the compressed length first, we will need to check again once confirming
        // that it contains snappy-compressed data (this will be the case for all non-p2p messages).
        if first_message_bytes.len() > MAX_PAYLOAD_SIZE {
            return Err(P2PStreamError::MessageTooBig {
                message_size: first_message_bytes.len(),
                max_size: MAX_PAYLOAD_SIZE,
            })
        }

        // The first message sent MUST be a hello OR disconnect message
        //
        // If the first message is a disconnect message, we should not decode using
        // Decodable::decode, because the first message (either Disconnect or Hello) is not snappy
        // compressed, and the Decodable implementation assumes that non-hello messages are snappy
        // compressed.
        let their_hello = match P2PMessage::decode(&mut &first_message_bytes[..]) {
            Ok(P2PMessage::Hello(hello)) => Ok(hello),
            Ok(P2PMessage::Disconnect(reason)) => {
                tracing::error!("Disconnected by peer during handshake: {}", reason);
                counter!("p2pstream.disconnected_errors", 1);
                Err(P2PStreamError::HandshakeError(P2PHandshakeError::Disconnected(reason)))
            }
            Err(err) => {
                tracing::warn!(?err, msg=%hex::encode(&first_message_bytes), "Failed to decode first message from peer");
                Err(P2PStreamError::HandshakeError(err.into()))
            }
            Ok(msg) => {
                tracing::error!("expected hello message but received: {:?}", msg);
                Err(P2PStreamError::HandshakeError(P2PHandshakeError::NonHelloMessageInHandshake))
            }
        }?;

        // TODO: explicitly document that we only support v5.
        if their_hello.protocol_version != ProtocolVersion::V5 {
            // TODO: do we want to send a `Disconnect` message here?
            return Err(P2PStreamError::MismatchedProtocolVersion {
                expected: ProtocolVersion::V5 as u8,
                got: their_hello.protocol_version as u8,
            })
        }

        // determine shared capabilities (currently returns only one capability)
        let capability =
            set_capability_offsets(hello.capabilities, their_hello.capabilities.clone())?;

        let stream = P2PStream::new(self.inner, capability);

        Ok((stream, their_hello))
    }
}

/// A P2PStream wraps over any `Stream` that yields bytes and makes it compatible with `p2p`
/// protocol messages.
#[pin_project]
#[derive(Debug)]
pub struct P2PStream<S> {
    #[pin]
    inner: S,

    /// The snappy encoder used for compressing outgoing messages
    encoder: snap::raw::Encoder,

    /// The snappy decoder used for decompressing incoming messages
    decoder: snap::raw::Decoder,

    /// The state machine used for keeping track of the peer's ping status.
    pinger: Pinger,

    /// The supported capability for this stream.
    shared_capability: SharedCapability,

    /// Outgoing messages buffered for sending to the underlying stream.
    outgoing_messages: VecDeque<Bytes>,

    /// Whether this stream is currently in the process of disconnecting by sending a disconnect
    /// message.
    disconnecting: bool,
}

impl<S> P2PStream<S> {
    /// Create a new [`P2PStream`] from the provided stream.
    /// New [`P2PStream`]s are assumed to have completed the `p2p` handshake successfully and are
    /// ready to send and receive subprotocol messages.
    pub fn new(inner: S, capability: SharedCapability) -> Self {
        Self {
            inner,
            encoder: snap::raw::Encoder::new(),
            decoder: snap::raw::Decoder::new(),
            pinger: Pinger::new(PING_INTERVAL, PING_TIMEOUT),
            shared_capability: capability,
            outgoing_messages: VecDeque::new(),
            disconnecting: false,
        }
    }

    /// Returns the shared capability for this stream.
    pub fn shared_capability(&self) -> &SharedCapability {
        &self.shared_capability
    }

    /// Returns `true` if the connection is about to disconnect.
    pub fn is_disconnecting(&self) -> bool {
        self.disconnecting
    }

    /// Starts to gracefully disconnect the connection by sending a Disconnect message and stop
    /// reading new messages.
    ///
    /// Once disconnect process has started, the [`Stream`] will terminate immediately.
    pub fn start_disconnect(&mut self, reason: DisconnectReason) {
        // clear any buffered messages and queue in
        self.outgoing_messages.clear();
        let disconnect = P2PMessage::Disconnect(reason);
        let mut buf = BytesMut::with_capacity(disconnect.length());
        disconnect.encode(&mut buf);
        self.outgoing_messages.push_back(buf.freeze());
        self.disconnecting = true;
    }
}

impl<S> P2PStream<S>
where
    S: Sink<Bytes, Error = io::Error> + Unpin,
{
    /// Disconnects the connection by sending a disconnect message.
    ///
    /// This future resolves once the disconnect message has been sent.
    pub async fn disconnect(mut self, reason: DisconnectReason) -> Result<(), P2PStreamError> {
        self.start_disconnect(reason);
        self.close().await
    }
}

// S must also be `Sink` because we need to be able to respond with ping messages to follow the
// protocol
impl<S> Stream for P2PStream<S>
where
    S: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
{
    type Item = Result<BytesMut, P2PStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.disconnecting {
            // if disconnecting, stop reading messages
            return Poll::Ready(None)
        }

        // poll the pinger to determine if we should send a ping
        match this.pinger.poll_ping(cx) {
            Poll::Pending => {}
            Poll::Ready(Ok(PingerEvent::Ping)) => {
                // encode the ping message
                let mut ping_bytes = BytesMut::new();
                P2PMessage::Ping.encode(&mut ping_bytes);

                // check if the buffer is full
                if this.outgoing_messages.len() >= MAX_P2P_CAPACITY {
                    return Poll::Ready(Some(Err(P2PStreamError::SendBufferFull)))
                }

                // if the sink is not ready, buffer the message
                this.outgoing_messages.push_back(ping_bytes.into());
            }
            _ => {
                // encode the disconnect message
                this.start_disconnect(DisconnectReason::PingTimeout);

                // End the stream after ping related error
                return Poll::Ready(None)
            }
        }

        // we should loop here to ensure we don't return Poll::Pending if we have a message to
        // return behind any pings we need to respond to
        while let Poll::Ready(res) = this.inner.poll_next_unpin(cx) {
            let bytes = match res {
                Some(Ok(bytes)) => bytes,
                Some(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
                None => return Poll::Ready(None),
            };

            let id = *bytes.first().ok_or(P2PStreamError::EmptyProtocolMessage)?;

            match id {
                _ if id == P2PMessageID::Ping as u8 => {
                    // we have received a ping, so we will send a pong
                    let pong = P2PMessage::Pong;
                    let mut pong_bytes = BytesMut::with_capacity(pong.length());
                    pong.encode(&mut pong_bytes);

                    // check if the buffer is full
                    if this.outgoing_messages.len() >= MAX_P2P_CAPACITY {
                        return Poll::Ready(Some(Err(P2PStreamError::SendBufferFull)))
                    }
                    // continue to the next message if there is one
                    this.outgoing_messages.push_back(pong_bytes.into());
                }
                _ if id == P2PMessageID::Disconnect as u8 => {
                    let reason = DisconnectReason::decode(&mut &bytes[1..]).map_err(|err| {
                        tracing::warn!(
                            ?err, msg=%hex::encode(&bytes[1..]), "Failed to decode disconnect message from peer"
                        );
                        err
                    })?;
                    return Poll::Ready(Some(Err(P2PStreamError::Disconnected(reason))))
                }
                _ if id == P2PMessageID::Hello as u8 => {
                    // we have received a hello message outside of the handshake, so we will return
                    // an error
                    return Poll::Ready(Some(Err(P2PStreamError::HandshakeError(
                        P2PHandshakeError::HelloNotInHandshake,
                    ))))
                }
                _ if id == P2PMessageID::Pong as u8 => {
                    // TODO: do we need to decode the pong?
                    // if we were waiting for a pong, this will reset the pinger state
                    this.pinger.on_pong()?
                }
                _ if id > MAX_P2P_MESSAGE_ID && id <= MAX_RESERVED_MESSAGE_ID => {
                    // we have received an unknown reserved message
                    return Poll::Ready(Some(Err(P2PStreamError::UnknownReservedMessageId(id))))
                }
                _ => {
                    // first check that the compressed message length does not exceed the max
                    // message size
                    let decompressed_len = snap::raw::decompress_len(&bytes[1..])?;
                    if decompressed_len > MAX_PAYLOAD_SIZE {
                        return Poll::Ready(Some(Err(P2PStreamError::MessageTooBig {
                            message_size: decompressed_len,
                            max_size: MAX_PAYLOAD_SIZE,
                        })))
                    }

                    // then decompress the message
                    let mut decompress_buf = BytesMut::zeroed(decompressed_len + 1);

                    // we have a subprotocol message that needs to be sent in the stream.
                    // first, switch the message id based on offset so the next layer can decode it
                    // without being aware of the p2p stream's state (shared capabilities / the
                    // message id offset)
                    decompress_buf[0] = bytes[0] - this.shared_capability.offset();
                    this.decoder.decompress(&bytes[1..], &mut decompress_buf[1..])?;

                    return Poll::Ready(Some(Ok(decompress_buf)))
                }
            }
        }

        Poll::Pending
    }
}

impl<S> Sink<Bytes> for P2PStream<S>
where
    S: Sink<Bytes, Error = io::Error> + Unpin,
{
    type Error = P2PStreamError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let mut this = self.as_mut();

        match this.inner.poll_ready_unpin(cx) {
            Poll::Pending => {}
            Poll::Ready(Err(err)) => return Poll::Ready(Err(P2PStreamError::Io(err))),
            Poll::Ready(Ok(())) => {
                let flushed = this.poll_flush(cx);
                if flushed.is_ready() {
                    return flushed
                }
            }
        }

        if self.outgoing_messages.len() < MAX_P2P_CAPACITY {
            // still has capacity
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        let this = self.project();

        // ensure we have free capacity
        if this.outgoing_messages.len() >= MAX_P2P_CAPACITY {
            return Err(P2PStreamError::SendBufferFull)
        }

        let mut compressed = BytesMut::zeroed(1 + snap::raw::max_compress_len(item.len() - 1));

        // all messages sent in this stream are subprotocol messages, so we need to switch the
        // message id based on the offset
        compressed[0] = item[0] + this.shared_capability.offset();
        let compressed_size = this.encoder.compress(&item[1..], &mut compressed[1..])?;

        // truncate the compressed buffer to the actual compressed size (plus one for the message
        // id)
        compressed.truncate(compressed_size + 1);

        this.outgoing_messages.push_back(compressed.freeze());

        Ok(())
    }

    /// Returns Poll::Ready(Ok(())) when no buffered items remain and the sink has been successfully
    /// closed.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let mut this = self.project();
        loop {
            match ready!(this.inner.as_mut().poll_flush(cx)) {
                Err(err) => return Poll::Ready(Err(err.into())),
                Ok(()) => {
                    if let Some(message) = this.outgoing_messages.pop_front() {
                        if let Err(err) = this.inner.as_mut().start_send(message) {
                            return Poll::Ready(Err(err.into()))
                        }
                    } else {
                        return Poll::Ready(Ok(()))
                    }
                }
            }
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(self.as_mut().poll_flush(cx))?;

        Poll::Ready(Ok(()))
    }
}

/// Determines the offsets for each shared capability between the input list of peer
/// capabilities and the input list of locally supported capabilities.
///
/// Currently only `eth` versions 66 and 67 are supported.
pub fn set_capability_offsets(
    local_capabilities: Vec<Capability>,
    peer_capabilities: Vec<Capability>,
) -> Result<SharedCapability, P2PStreamError> {
    // find intersection of capabilities
    let our_capabilities_map =
        local_capabilities.into_iter().map(|c| (c.name, c.version)).collect::<HashMap<_, _>>();

    // map of capability name to version
    let mut shared_capabilities = HashMap::new();

    // The `Ord` implementation for capability names should be equivalent to geth (and every other
    // client), since geth uses golang's default string comparison, which orders strings
    // lexicographically.
    // https://golang.org/pkg/strings/#Compare
    //
    // This is important because the capability name is used to determine the message id offset, so
    // if the sorting is not identical, offsets for connected peers could be inconsistent.
    // This would cause the peers to send messages with the wrong message id, which is usually a
    // protocol violation.
    //
    // The `Ord` implementation for `SmolStr` (used here) currently delegates to rust's `Ord`
    // implementation for `str`, which also orders strings lexicographically.
    let mut shared_capability_names = BTreeSet::new();

    // find highest shared version of each shared capability
    for capability in peer_capabilities {
        // if this is Some, we share this capability
        if let Some(version) = our_capabilities_map.get(&capability.name) {
            // If multiple versions are shared of the same (equal name) capability, the numerically
            // highest wins, others are ignored
            if capability.version <= *version {
                shared_capabilities.insert(capability.name.clone(), capability.version);
                shared_capability_names.insert(capability.name);
            }
        }
    }

    // disconnect if we don't share any capabilities
    if shared_capabilities.is_empty() {
        // TODO: send a disconnect message? if we want to do this, this will need to be a member
        // method of `UnauthedP2PStream` so it can access the inner stream
        return Err(P2PStreamError::HandshakeError(P2PHandshakeError::NoSharedCapabilities))
    }

    // order versions based on capability name (alphabetical) and select offsets based on
    // BASE_OFFSET + prev_total_message
    let mut shared_with_offsets = Vec::new();

    // Message IDs are assumed to be compact from ID 0x10 onwards (0x00-0x0f is reserved for the
    // "p2p" capability) and given to each shared (equal-version, equal-name) capability in
    // alphabetic order.
    let mut offset = MAX_RESERVED_MESSAGE_ID + 1;
    for name in shared_capability_names {
        let version = shared_capabilities.get(&name).unwrap();

        let shared_capability = SharedCapability::new(&name, *version as u8, offset)?;

        match shared_capability {
            SharedCapability::UnknownCapability { .. } => {
                // Capabilities which are not shared are ignored
                tracing::warn!("unknown capability: name={:?}, version={}", name, version,);
            }
            SharedCapability::Eth { .. } => {
                shared_with_offsets.push(shared_capability.clone());

                // increment the offset if the capability is known
                offset += shared_capability.num_messages()?;
            }
        }
    }

    // TODO: support multiple capabilities - we would need a new Stream type to go on top of
    // `P2PStream` containing its capability. `P2PStream` would still send pings and handle
    // pongs, but instead contain a map of capabilities to their respective stream / channel.
    // Each channel would be responsible for containing the offset for that stream and would
    // only increment / decrement message IDs.
    // NOTE: since the `P2PStream` currently only supports one capability, we set the
    // capability with the lowest offset.
    Ok(shared_with_offsets
        .first()
        .ok_or(P2PStreamError::HandshakeError(P2PHandshakeError::NoSharedCapabilities))?
        .clone())
}

/// This represents only the reserved `p2p` subprotocol messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum P2PMessage {
    /// The first packet sent over the connection, and sent once by both sides.
    Hello(HelloMessage),

    /// Inform the peer that a disconnection is imminent; if received, a peer should disconnect
    /// immediately.
    Disconnect(DisconnectReason),

    /// Requests an immediate reply of [`Pong`] from the peer.
    Ping,

    /// Reply to the peer's [`Ping`] packet.
    Pong,
}

impl P2PMessage {
    /// Gets the [`P2PMessageID`] for the given message.
    pub fn message_id(&self) -> P2PMessageID {
        match self {
            P2PMessage::Hello(_) => P2PMessageID::Hello,
            P2PMessage::Disconnect(_) => P2PMessageID::Disconnect,
            P2PMessage::Ping => P2PMessageID::Ping,
            P2PMessage::Pong => P2PMessageID::Pong,
        }
    }
}

/// The [`Encodable`](reth_rlp::Encodable) implementation for [`P2PMessage::Ping`] and
/// [`P2PMessage::Pong`] encodes the message as RLP, and prepends a snappy header to the RLP bytes
/// for all variants except the [`P2PMessage::Hello`] variant, because the hello message is never
/// compressed in the `p2p` subprotocol.
impl Encodable for P2PMessage {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        (self.message_id() as u8).encode(out);
        match self {
            P2PMessage::Hello(msg) => msg.encode(out),
            P2PMessage::Disconnect(msg) => msg.encode(out),
            P2PMessage::Ping => {
                out.put_u8(0x01);
                out.put_u8(0x00);
                out.put_u8(EMPTY_STRING_CODE);
            }
            P2PMessage::Pong => {
                out.put_u8(0x01);
                out.put_u8(0x00);
                out.put_u8(EMPTY_STRING_CODE);
            }
        }
    }

    fn length(&self) -> usize {
        let payload_len = match self {
            P2PMessage::Hello(msg) => msg.length(),
            P2PMessage::Disconnect(msg) => msg.length(),
            P2PMessage::Ping => 3, // len([0x01, 0x00, 0x80]) = 3
            P2PMessage::Pong => 3, // len([0x01, 0x00, 0x80]) = 3
        };
        payload_len + 1 // (1 for length of p2p message id)
    }
}

/// The [`Decodable`](reth_rlp::Decodable) implementation for [`P2PMessage`] assumes that each of
/// the message variants are snappy compressed, except for the [`P2PMessage::Hello`] variant since
/// the hello message is never compressed in the `p2p` subprotocol.
impl Decodable for P2PMessage {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let message_id = u8::decode(&mut &buf[..])?;
        let id = P2PMessageID::try_from(message_id)
            .or(Err(DecodeError::Custom("unknown p2p message id")))?;
        buf.advance(1);
        match id {
            P2PMessageID::Hello => Ok(P2PMessage::Hello(HelloMessage::decode(buf)?)),
            P2PMessageID::Disconnect => Ok(P2PMessage::Disconnect(DisconnectReason::decode(buf)?)),
            P2PMessageID::Ping => {
                // len([0x01, 0x00, 0x80]) = 3
                buf.advance(3);
                Ok(P2PMessage::Ping)
            }
            P2PMessageID::Pong => {
                // len([0x01, 0x00, 0x80]) = 3
                buf.advance(3);
                Ok(P2PMessage::Pong)
            }
        }
    }
}

/// Message IDs for `p2p` subprotocol messages.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum P2PMessageID {
    /// Message ID for the [`P2PMessage::Hello`] message.
    Hello = 0x00,

    /// Message ID for the [`P2PMessage::Disconnect`] message.
    Disconnect = 0x01,

    /// Message ID for the [`P2PMessage::Ping`] message.
    Ping = 0x02,

    /// Message ID for the [`P2PMessage::Pong`] message.
    Pong = 0x03,
}

impl From<P2PMessage> for P2PMessageID {
    fn from(msg: P2PMessage) -> Self {
        match msg {
            P2PMessage::Hello(_) => P2PMessageID::Hello,
            P2PMessage::Disconnect(_) => P2PMessageID::Disconnect,
            P2PMessage::Ping => P2PMessageID::Ping,
            P2PMessage::Pong => P2PMessageID::Pong,
        }
    }
}

impl TryFrom<u8> for P2PMessageID {
    type Error = P2PStreamError;

    fn try_from(id: u8) -> Result<Self, Self::Error> {
        match id {
            0x00 => Ok(P2PMessageID::Hello),
            0x01 => Ok(P2PMessageID::Disconnect),
            0x02 => Ok(P2PMessageID::Ping),
            0x03 => Ok(P2PMessageID::Pong),
            _ => Err(P2PStreamError::UnknownReservedMessageId(id)),
        }
    }
}

/// RLPx `p2p` protocol version
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolVersion {
    /// `p2p` version 4
    V4 = 4,
    /// `p2p` version 5
    V5 = 5,
}

impl Encodable for ProtocolVersion {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        (*self as u8).encode(out)
    }
    fn length(&self) -> usize {
        // the version should be a single byte
        (*self as u8).length()
    }
}

impl Decodable for ProtocolVersion {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let version = u8::decode(buf)?;
        match version {
            4 => Ok(ProtocolVersion::V4),
            5 => Ok(ProtocolVersion::V5),
            _ => Err(DecodeError::Custom("unknown p2p protocol version")),
        }
    }
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        ProtocolVersion::V5
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DisconnectReason, EthVersion};
    use reth_ecies::util::pk2id;
    use reth_rlp::EMPTY_STRING_CODE;
    use secp256k1::{SecretKey, SECP256K1};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_util::codec::Decoder;

    /// Returns a testing `HelloMessage` and new secretkey
    fn eth_hello() -> (HelloMessage, SecretKey) {
        let server_key = SecretKey::new(&mut rand::thread_rng());
        let hello = HelloMessage {
            protocol_version: ProtocolVersion::V5,
            client_version: "bitcoind/1.0.0".to_string(),
            capabilities: vec![EthVersion::Eth67.into()],
            port: 30303,
            id: pk2id(&server_key.public_key(SECP256K1)),
        };
        (hello, server_key)
    }

    #[tokio::test]
    async fn test_can_disconnect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let expected_disconnect = DisconnectReason::UselessPeer;

        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = crate::PassthroughCodec::default().framed(incoming);

            let (server_hello, _) = eth_hello();

            let (p2p_stream, _) =
                UnauthedP2PStream::new(stream).handshake(server_hello).await.unwrap();

            p2p_stream.disconnect(expected_disconnect).await.unwrap();
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = crate::PassthroughCodec::default().framed(outgoing);

        let (client_hello, _) = eth_hello();

        let (mut p2p_stream, _) =
            UnauthedP2PStream::new(sink).handshake(client_hello).await.unwrap();

        let err = p2p_stream.next().await.unwrap().unwrap_err();
        match err {
            P2PStreamError::Disconnected(reason) => assert_eq!(reason, expected_disconnect),
            _ => panic!("unexpected err"),
        }
    }

    #[tokio::test]
    async fn test_handshake_passthrough() {
        // create a p2p stream and server, then confirm that the two are authed
        // create tcpstream
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = crate::PassthroughCodec::default().framed(incoming);

            let (server_hello, _) = eth_hello();

            let unauthed_stream = UnauthedP2PStream::new(stream);
            let (p2p_stream, _) = unauthed_stream.handshake(server_hello).await.unwrap();

            // ensure that the two share a single capability, eth67
            assert_eq!(
                p2p_stream.shared_capability,
                SharedCapability::Eth {
                    version: EthVersion::Eth67,
                    offset: MAX_RESERVED_MESSAGE_ID + 1
                }
            );
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = crate::PassthroughCodec::default().framed(outgoing);

        let (client_hello, _) = eth_hello();

        let unauthed_stream = UnauthedP2PStream::new(sink);
        let (p2p_stream, _) = unauthed_stream.handshake(client_hello).await.unwrap();

        // ensure that the two share a single capability, eth67
        assert_eq!(
            p2p_stream.shared_capability,
            SharedCapability::Eth {
                version: EthVersion::Eth67,
                offset: MAX_RESERVED_MESSAGE_ID + 1
            }
        );

        // make sure the server receives the message and asserts before ending the test
        handle.await.unwrap();
    }

    #[test]
    fn test_ping_snappy_encoding_parity() {
        // encode ping using our `Encodable` implementation
        let ping = P2PMessage::Ping;
        let mut ping_encoded = Vec::new();
        ping.encode(&mut ping_encoded);

        // the definition of ping is 0x80 (an empty rlp string)
        let ping_raw = vec![EMPTY_STRING_CODE];
        let mut snappy_encoder = snap::raw::Encoder::new();
        let ping_compressed = snappy_encoder.compress_vec(&ping_raw).unwrap();
        let mut ping_expected = vec![P2PMessageID::Ping as u8];
        ping_expected.extend(&ping_compressed);

        // ensure that the two encodings are equal
        assert_eq!(
            ping_expected, ping_encoded,
            "left: {ping_expected:#x?}, right: {ping_encoded:#x?}"
        );

        // also ensure that the length is correct
        assert_eq!(ping_expected.len(), P2PMessage::Ping.length());

        // try to decode using Decodable
        let p2p_message = P2PMessage::decode(&mut &ping_expected[..]).unwrap();
        assert_eq!(p2p_message, P2PMessage::Ping);

        // finally decode the encoded message with snappy
        let mut snappy_decoder = snap::raw::Decoder::new();

        // the message id is not compressed, only compress the latest bits
        let decompressed = snappy_decoder.decompress_vec(&ping_encoded[1..]).unwrap();

        assert_eq!(decompressed, ping_raw);
    }
}
