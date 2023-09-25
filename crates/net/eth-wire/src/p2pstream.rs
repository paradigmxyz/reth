#![allow(dead_code, unreachable_pub, missing_docs, unused_variables)]
use crate::{
    capability::{Capability, SharedCapability},
    disconnect::CanDisconnect,
    errors::{P2PHandshakeError, P2PStreamError},
    pinger::{Pinger, PingerEvent},
    DisconnectReason, HelloMessage,
};
use alloy_rlp::{Decodable, Encodable, Error as RlpError, EMPTY_LIST_CODE};
use futures::{Sink, SinkExt, StreamExt};
use pin_project::pin_project;
use reth_codecs::derive_arbitrary;
use reth_metrics::metrics::counter;
use reth_primitives::{
    bytes::{Buf, BufMut, Bytes, BytesMut},
    hex,
};
use std::{
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    io,
    pin::Pin,
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio_stream::Stream;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

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
///
/// Note: this default is rather low because it is expected that the [P2PStream] wraps an
/// [ECIESStream](reth_ecies::stream::ECIESStream) which internally already buffers a few MB of
/// encoded data.
const MAX_P2P_CAPACITY: usize = 2;

/// An un-authenticated [`P2PStream`]. This is consumed and returns a [`P2PStream`] after the
/// `Hello` handshake is completed.
#[pin_project]
#[derive(Debug)]
pub struct UnauthedP2PStream<S> {
    #[pin]
    inner: S,
}

impl<S> UnauthedP2PStream<S> {
    /// Create a new `UnauthedP2PStream` from a type `S` which implements `Stream` and `Sink`.
    pub fn new(inner: S) -> Self {
        Self { inner }
    }

    /// Returns a reference to the inner stream.
    pub fn inner(&self) -> &S {
        &self.inner
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
        tracing::trace!(?hello, "sending p2p hello to peer");

        // send our hello message with the Sink
        let mut raw_hello_bytes = BytesMut::new();
        P2PMessage::Hello(hello.clone()).encode(&mut raw_hello_bytes);
        self.inner.send(raw_hello_bytes.into()).await?;

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
                tracing::debug!("Disconnected by peer during handshake: {}", reason);
                counter!("p2pstream.disconnected_errors", 1);
                Err(P2PStreamError::HandshakeError(P2PHandshakeError::Disconnected(reason)))
            }
            Err(err) => {
                tracing::debug!(?err, msg=%hex::encode(&first_message_bytes), "Failed to decode first message from peer");
                Err(P2PStreamError::HandshakeError(err.into()))
            }
            Ok(msg) => {
                tracing::debug!("expected hello message but received: {:?}", msg);
                Err(P2PStreamError::HandshakeError(P2PHandshakeError::NonHelloMessageInHandshake))
            }
        }?;

        tracing::trace!(
            hello=?their_hello,
            "validating incoming p2p hello from peer"
        );

        if (hello.protocol_version as u8) != their_hello.protocol_version as u8 {
            // send a disconnect message notifying the peer of the protocol version mismatch
            self.send_disconnect(DisconnectReason::IncompatibleP2PProtocolVersion).await?;
            return Err(P2PStreamError::MismatchedProtocolVersion {
                expected: hello.protocol_version as u8,
                got: their_hello.protocol_version as u8,
            })
        }

        // determine shared capabilities (currently returns only one capability)
        let capability_res =
            set_capability_offsets(hello.capabilities, their_hello.capabilities.clone());

        let shared_capability = match capability_res {
            Err(err) => {
                // we don't share any capabilities, send a disconnect message
                self.send_disconnect(DisconnectReason::UselessPeer).await?;
                Err(err)
            }
            Ok(cap) => Ok(cap),
        }?;

        let stream = P2PStream::new(self.inner, shared_capability);

        Ok((stream, their_hello))
    }
}

impl<S> UnauthedP2PStream<S>
where
    S: Sink<Bytes, Error = io::Error> + Unpin,
{
    /// Send a disconnect message during the handshake. This is sent without snappy compression.
    pub async fn send_disconnect(
        &mut self,
        reason: DisconnectReason,
    ) -> Result<(), P2PStreamError> {
        let mut buf = BytesMut::new();
        P2PMessage::Disconnect(reason).encode(&mut buf);
        tracing::trace!(
            %reason,
            "Sending disconnect message during the handshake",
        );
        self.inner.send(buf.freeze()).await.map_err(P2PStreamError::Io)
    }
}

#[async_trait::async_trait]
impl<S> CanDisconnect<Bytes> for P2PStream<S>
where
    S: Sink<Bytes, Error = io::Error> + Unpin + Send + Sync,
{
    async fn disconnect(&mut self, reason: DisconnectReason) -> Result<(), P2PStreamError> {
        self.disconnect(reason).await
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

    /// Maximum number of messages that we can buffer here before the [Sink] impl returns
    /// [Poll::Pending].
    outgoing_message_buffer_capacity: usize,

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
            outgoing_message_buffer_capacity: MAX_P2P_CAPACITY,
            disconnecting: false,
        }
    }

    /// Returns a reference to the inner stream.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Sets a custom outgoing message buffer capacity.
    ///
    /// # Panics
    ///
    /// If the provided capacity is `0`.
    pub fn set_outgoing_message_buffer_capacity(&mut self, capacity: usize) {
        self.outgoing_message_buffer_capacity = capacity;
    }

    /// Returns the shared capability for this stream.
    pub fn shared_capability(&self) -> &SharedCapability {
        &self.shared_capability
    }

    /// Returns `true` if the connection is about to disconnect.
    pub fn is_disconnecting(&self) -> bool {
        self.disconnecting
    }

    /// Returns `true` if the stream has outgoing capacity.
    fn has_outgoing_capacity(&self) -> bool {
        self.outgoing_messages.len() < self.outgoing_message_buffer_capacity
    }

    /// Queues in a _snappy_ encoded [`P2PMessage::Pong`] message.
    fn send_pong(&mut self) {
        let pong = P2PMessage::Pong;
        let mut pong_bytes = BytesMut::with_capacity(pong.length());
        pong.encode(&mut pong_bytes);
        self.outgoing_messages.push_back(pong_bytes.freeze());
    }

    /// Queues in a _snappy_ encoded [`P2PMessage::Ping`] message.
    fn send_ping(&mut self) {
        let ping = P2PMessage::Ping;
        let mut ping_bytes = BytesMut::with_capacity(ping.length());
        ping.encode(&mut ping_bytes);
        self.outgoing_messages.push_back(ping_bytes.freeze());
    }

    /// Starts to gracefully disconnect the connection by sending a Disconnect message and stop
    /// reading new messages.
    ///
    /// Once disconnect process has started, the [`Stream`] will terminate immediately.
    ///
    /// # Errors
    ///
    /// Returns an error only if the message fails to compress.
    pub fn start_disconnect(&mut self, reason: DisconnectReason) -> Result<(), snap::Error> {
        // clear any buffered messages and queue in
        self.outgoing_messages.clear();
        let disconnect = P2PMessage::Disconnect(reason);
        let mut buf = BytesMut::with_capacity(disconnect.length());
        disconnect.encode(&mut buf);

        let mut compressed = BytesMut::zeroed(1 + snap::raw::max_compress_len(buf.len() - 1));
        let compressed_size =
            self.encoder.compress(&buf[1..], &mut compressed[1..]).map_err(|err| {
                tracing::debug!(
                    ?err,
                    msg=%hex::encode(&buf[1..]),
                    "error compressing disconnect"
                );
                err
            })?;

        // truncate the compressed buffer to the actual compressed size (plus one for the message
        // id)
        compressed.truncate(compressed_size + 1);

        // we do not add the capability offset because the disconnect message is a `p2p` reserved
        // message
        compressed[0] = buf[0];

        self.outgoing_messages.push_back(compressed.freeze());
        self.disconnecting = true;
        Ok(())
    }
}

impl<S> P2PStream<S>
where
    S: Sink<Bytes, Error = io::Error> + Unpin + Send,
{
    /// Disconnects the connection by sending a disconnect message.
    ///
    /// This future resolves once the disconnect message has been sent and the stream has been
    /// closed.
    pub async fn disconnect(&mut self, reason: DisconnectReason) -> Result<(), P2PStreamError> {
        self.start_disconnect(reason)?;
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

        // we should loop here to ensure we don't return Poll::Pending if we have a message to
        // return behind any pings we need to respond to
        while let Poll::Ready(res) = this.inner.poll_next_unpin(cx) {
            let bytes = match res {
                Some(Ok(bytes)) => bytes,
                Some(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
                None => return Poll::Ready(None),
            };

            // first check that the compressed message length does not exceed the max
            // payload size
            let decompressed_len = snap::raw::decompress_len(&bytes[1..])?;
            if decompressed_len > MAX_PAYLOAD_SIZE {
                return Poll::Ready(Some(Err(P2PStreamError::MessageTooBig {
                    message_size: decompressed_len,
                    max_size: MAX_PAYLOAD_SIZE,
                })))
            }

            // create a buffer to hold the decompressed message, adding a byte to the length for
            // the message ID byte, which is the first byte in this buffer
            let mut decompress_buf = BytesMut::zeroed(decompressed_len + 1);

            // each message following a successful handshake is compressed with snappy, so we need
            // to decompress the message before we can decode it.
            this.decoder.decompress(&bytes[1..], &mut decompress_buf[1..]).map_err(|err| {
                tracing::debug!(
                    ?err,
                    msg=%hex::encode(&bytes[1..]),
                    "error decompressing p2p message"
                );
                err
            })?;

            let id = *bytes.first().ok_or(P2PStreamError::EmptyProtocolMessage)?;
            match id {
                _ if id == P2PMessageID::Ping as u8 => {
                    tracing::trace!("Received Ping, Sending Pong");
                    this.send_pong();
                    // This is required because the `Sink` may not be polled externally, and if
                    // that happens, the pong will never be sent.
                    cx.waker().wake_by_ref();
                }
                _ if id == P2PMessageID::Disconnect as u8 => {
                    let reason = DisconnectReason::decode(&mut &decompress_buf[1..]).map_err(|err| {
                        tracing::debug!(
                            ?err, msg=%hex::encode(&decompress_buf[1..]), "Failed to decode disconnect message from peer"
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
                    // if we were waiting for a pong, this will reset the pinger state
                    this.pinger.on_pong()?
                }
                _ if id > MAX_P2P_MESSAGE_ID && id <= MAX_RESERVED_MESSAGE_ID => {
                    // we have received an unknown reserved message
                    return Poll::Ready(Some(Err(P2PStreamError::UnknownReservedMessageId(id))))
                }
                _ => {
                    // we have received a message that is outside the `p2p` reserved message space,
                    // so it is a subprotocol message.

                    // Peers must be able to identify messages meant for different subprotocols
                    // using a single message ID byte, and those messages must be distinct from the
                    // lower-level `p2p` messages.
                    //
                    // To ensure that messages for subprotocols are distinct from messages meant
                    // for the `p2p` capability, message IDs 0x00 - 0x0f are reserved for `p2p`
                    // messages, so subprotocol messages must have an ID of 0x10 or higher.
                    //
                    // To ensure that messages for two different capabilities are distinct from
                    // each other, all shared capabilities are first ordered lexicographically.
                    // Message IDs are then reserved in this order, starting at 0x10, reserving a
                    // message ID for each message the capability supports.
                    //
                    // For example, if the shared capabilities are `eth/67` (containing 10
                    // messages), and "qrs/65" (containing 8 messages):
                    //
                    //  * The special case of `p2p`: `p2p` is reserved message IDs 0x00 - 0x0f.
                    //  * `eth/67` is reserved message IDs 0x10 - 0x19.
                    //  * `qrs/65` is reserved message IDs 0x1a - 0x21.
                    //
                    decompress_buf[0] = bytes[0] - this.shared_capability.offset();

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

        // poll the pinger to determine if we should send a ping
        match this.pinger.poll_ping(cx) {
            Poll::Pending => {}
            Poll::Ready(Ok(PingerEvent::Ping)) => {
                this.send_ping();
            }
            _ => {
                // encode the disconnect message
                this.start_disconnect(DisconnectReason::PingTimeout)?;

                // End the stream after ping related error
                return Poll::Ready(Ok(()))
            }
        }

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

        if self.has_outgoing_capacity() {
            // still has capacity
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        // ensure we have free capacity
        if !self.has_outgoing_capacity() {
            return Err(P2PStreamError::SendBufferFull)
        }

        let this = self.project();

        let mut compressed = BytesMut::zeroed(1 + snap::raw::max_compress_len(item.len() - 1));
        let compressed_size =
            this.encoder.compress(&item[1..], &mut compressed[1..]).map_err(|err| {
                tracing::debug!(
                    ?err,
                    msg=%hex::encode(&item[1..]),
                    "error compressing p2p message"
                );
                err
            })?;

        // truncate the compressed buffer to the actual compressed size (plus one for the message
        // id)
        compressed.truncate(compressed_size + 1);

        // all messages sent in this stream are subprotocol messages, so we need to switch the
        // message id based on the offset
        compressed[0] = item[0] + this.shared_capability.offset();
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
                    let Some(message) = this.outgoing_messages.pop_front() else {
                        return Poll::Ready(Ok(()))
                    };
                    if let Err(err) = this.inner.as_mut().start_send(message) {
                        return Poll::Ready(Err(err.into()))
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
/// Additionally, the `p2p` capability version 5 is supported, but is
/// expected _not_ to be in neither `local_capabilities` or `peer_capabilities`.
pub fn set_capability_offsets(
    local_capabilities: Vec<Capability>,
    peer_capabilities: Vec<Capability>,
) -> Result<SharedCapability, P2PStreamError> {
    // find intersection of capabilities
    let our_capabilities = local_capabilities.into_iter().collect::<HashSet<_>>();

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
    for peer_capability in peer_capabilities {
        // if this is Some, we share this capability
        if our_capabilities.contains(&peer_capability) {
            // If multiple versions are shared of the same (equal name) capability, the numerically
            // highest wins, others are ignored

            let version = shared_capabilities.get(&peer_capability.name);
            if version.is_none() ||
                (version.is_some() && peer_capability.version > *version.expect("is some; qed"))
            {
                shared_capabilities.insert(peer_capability.name.clone(), peer_capability.version);
                shared_capability_names.insert(peer_capability.name);
            }
        }
    }

    // disconnect if we don't share any capabilities
    if shared_capabilities.is_empty() {
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
                tracing::debug!("unknown capability: name={:?}, version={}", name, version,);
            }
            SharedCapability::Eth { .. } => {
                // increment the offset if the capability is known
                offset += shared_capability.num_messages()?;

                shared_with_offsets.push(shared_capability);
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
#[derive_arbitrary(rlp)]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum P2PMessage {
    /// The first packet sent over the connection, and sent once by both sides.
    Hello(HelloMessage),

    /// Inform the peer that a disconnection is imminent; if received, a peer should disconnect
    /// immediately.
    Disconnect(DisconnectReason),

    /// Requests an immediate reply of [`P2PMessage::Pong`] from the peer.
    Ping,

    /// Reply to the peer's [`P2PMessage::Ping`] packet.
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

/// The [`Encodable`] implementation for [`P2PMessage::Ping`] and [`P2PMessage::Pong`] encodes the
/// message as RLP, and prepends a snappy header to the RLP bytes for all variants except the
/// [`P2PMessage::Hello`] variant, because the hello message is never compressed in the `p2p`
/// subprotocol.
impl Encodable for P2PMessage {
    fn encode(&self, out: &mut dyn BufMut) {
        (self.message_id() as u8).encode(out);
        match self {
            P2PMessage::Hello(msg) => msg.encode(out),
            P2PMessage::Disconnect(msg) => msg.encode(out),
            P2PMessage::Ping => {
                // Ping payload is _always_ snappy encoded
                out.put_u8(0x01);
                out.put_u8(0x00);
                out.put_u8(EMPTY_LIST_CODE);
            }
            P2PMessage::Pong => {
                // Pong payload is _always_ snappy encoded
                out.put_u8(0x01);
                out.put_u8(0x00);
                out.put_u8(EMPTY_LIST_CODE);
            }
        }
    }

    fn length(&self) -> usize {
        let payload_len = match self {
            P2PMessage::Hello(msg) => msg.length(),
            P2PMessage::Disconnect(msg) => msg.length(),
            // id + snappy encoded payload
            P2PMessage::Ping => 3, // len([0x01, 0x00, 0xc0]) = 3
            P2PMessage::Pong => 3, // len([0x01, 0x00, 0xc0]) = 3
        };
        payload_len + 1 // (1 for length of p2p message id)
    }
}

/// The [`Decodable`] implementation for [`P2PMessage`] assumes that each of the message variants
/// are snappy compressed, except for the [`P2PMessage::Hello`] variant since the hello message is
/// never compressed in the `p2p` subprotocol.
///
/// The [`Decodable`] implementation for [`P2PMessage::Ping`] and [`P2PMessage::Pong`] expects a
/// snappy encoded payload, see [`Encodable`] implementation.
impl Decodable for P2PMessage {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        /// Removes the snappy prefix from the Ping/Pong buffer
        fn advance_snappy_ping_pong_payload(buf: &mut &[u8]) -> alloy_rlp::Result<()> {
            if buf.len() < 3 {
                return Err(RlpError::InputTooShort)
            }
            if buf[..3] != [0x01, 0x00, EMPTY_LIST_CODE] {
                return Err(RlpError::Custom("expected snappy payload"))
            }
            buf.advance(3);
            Ok(())
        }

        let message_id = u8::decode(&mut &buf[..])?;
        let id = P2PMessageID::try_from(message_id)
            .or(Err(RlpError::Custom("unknown p2p message id")))?;
        buf.advance(1);
        match id {
            P2PMessageID::Hello => Ok(P2PMessage::Hello(HelloMessage::decode(buf)?)),
            P2PMessageID::Disconnect => Ok(P2PMessage::Disconnect(DisconnectReason::decode(buf)?)),
            P2PMessageID::Ping => {
                advance_snappy_ping_pong_payload(buf)?;
                Ok(P2PMessage::Ping)
            }
            P2PMessageID::Pong => {
                advance_snappy_ping_pong_payload(buf)?;
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
#[derive_arbitrary(rlp)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum ProtocolVersion {
    /// `p2p` version 4
    V4 = 4,
    /// `p2p` version 5
    #[default]
    V5 = 5,
}

impl Encodable for ProtocolVersion {
    fn encode(&self, out: &mut dyn BufMut) {
        (*self as u8).encode(out)
    }
    fn length(&self) -> usize {
        // the version should be a single byte
        (*self as u8).length()
    }
}

impl Decodable for ProtocolVersion {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let version = u8::decode(buf)?;
        match version {
            4 => Ok(ProtocolVersion::V4),
            5 => Ok(ProtocolVersion::V5),
            _ => Err(RlpError::Custom("unknown p2p protocol version")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DisconnectReason, EthVersion};
    use reth_discv4::DEFAULT_DISCOVERY_PORT;
    use reth_ecies::util::pk2id;
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
            port: DEFAULT_DISCOVERY_PORT,
            id: pk2id(&server_key.public_key(SECP256K1)),
        };
        (hello, server_key)
    }

    #[tokio::test]
    async fn test_can_disconnect() {
        reth_tracing::init_test_tracing();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let expected_disconnect = DisconnectReason::UselessPeer;

        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = crate::PassthroughCodec::default().framed(incoming);

            let (server_hello, _) = eth_hello();

            let (mut p2p_stream, _) =
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
            e => panic!("unexpected err: {e}"),
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

    #[tokio::test]
    async fn test_handshake_disconnect() {
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
            match unauthed_stream.handshake(server_hello.clone()).await {
                Ok((_, hello)) => {
                    panic!("expected handshake to fail, instead got a successful Hello: {hello:?}")
                }
                Err(P2PStreamError::MismatchedProtocolVersion { expected, got }) => {
                    assert_eq!(expected, server_hello.protocol_version as u8);
                    assert_ne!(expected, got);
                }
                Err(other_err) => {
                    panic!("expected mismatched protocol version error, got {other_err:?}")
                }
            }
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = crate::PassthroughCodec::default().framed(outgoing);

        let (mut client_hello, _) = eth_hello();

        // modify the hello to include an incompatible p2p protocol version
        client_hello.protocol_version = ProtocolVersion::V4;

        let unauthed_stream = UnauthedP2PStream::new(sink);
        match unauthed_stream.handshake(client_hello.clone()).await {
            Ok((_, hello)) => {
                panic!("expected handshake to fail, instead got a successful Hello: {hello:?}")
            }
            Err(P2PStreamError::MismatchedProtocolVersion { expected, got }) => {
                assert_eq!(expected, client_hello.protocol_version as u8);
                assert_ne!(expected, got);
            }
            Err(other_err) => {
                panic!("expected mismatched protocol version error, got {other_err:?}")
            }
        }

        // make sure the server receives the message and asserts before ending the test
        handle.await.unwrap();
    }

    #[test]
    fn test_peer_lower_capability_version() {
        let local_capabilities: Vec<Capability> =
            vec![EthVersion::Eth66.into(), EthVersion::Eth67.into(), EthVersion::Eth68.into()];
        let peer_capabilities: Vec<Capability> = vec![EthVersion::Eth66.into()];

        let shared_capability =
            set_capability_offsets(local_capabilities, peer_capabilities).unwrap();

        assert_eq!(
            shared_capability,
            SharedCapability::Eth {
                version: EthVersion::Eth66,
                offset: MAX_RESERVED_MESSAGE_ID + 1
            }
        )
    }

    #[test]
    fn test_peer_capability_version_too_low() {
        let local_capabilities: Vec<Capability> = vec![EthVersion::Eth67.into()];
        let peer_capabilities: Vec<Capability> = vec![EthVersion::Eth66.into()];

        let shared_capability = set_capability_offsets(local_capabilities, peer_capabilities);

        assert!(matches!(
            shared_capability,
            Err(P2PStreamError::HandshakeError(P2PHandshakeError::NoSharedCapabilities))
        ))
    }

    #[test]
    fn test_peer_capability_version_too_high() {
        let local_capabilities: Vec<Capability> = vec![EthVersion::Eth66.into()];
        let peer_capabilities: Vec<Capability> = vec![EthVersion::Eth67.into()];

        let shared_capability = set_capability_offsets(local_capabilities, peer_capabilities);

        assert!(matches!(
            shared_capability,
            Err(P2PStreamError::HandshakeError(P2PHandshakeError::NoSharedCapabilities))
        ))
    }

    #[test]
    fn snappy_decode_encode_ping() {
        let snappy_ping = b"\x02\x01\0\xc0";
        let ping = P2PMessage::decode(&mut &snappy_ping[..]).unwrap();
        assert!(matches!(ping, P2PMessage::Ping));

        let mut buf = BytesMut::with_capacity(ping.length());
        ping.encode(&mut buf);
        assert_eq!(buf.as_ref(), &snappy_ping[..]);
    }

    #[test]
    fn snappy_decode_encode_pong() {
        let snappy_pong = b"\x03\x01\0\xc0";
        let pong = P2PMessage::decode(&mut &snappy_pong[..]).unwrap();
        assert!(matches!(pong, P2PMessage::Pong));

        let mut buf = BytesMut::with_capacity(pong.length());
        pong.encode(&mut buf);
        assert_eq!(buf.as_ref(), &snappy_pong[..]);
    }
}
