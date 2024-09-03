use crate::{
    capability::SharedCapabilities,
    disconnect::CanDisconnect,
    errors::{P2PHandshakeError, P2PStreamError},
    pinger::{Pinger, PingerEvent},
    DisconnectReason, HelloMessage, HelloMessageWithProtocols,
};
use alloy_rlp::{Decodable, Encodable, Error as RlpError, EMPTY_LIST_CODE};
use futures::{Sink, SinkExt, StreamExt};
use pin_project::pin_project;
use reth_codecs::add_arbitrary_tests;
use reth_metrics::metrics::counter;
use reth_primitives::{
    bytes::{Buf, BufMut, Bytes, BytesMut},
    hex, GotExpected,
};
use std::{
    collections::VecDeque,
    io,
    pin::Pin,
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio_stream::Stream;
use tracing::{debug, trace};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// [`MAX_PAYLOAD_SIZE`] is the maximum size of an uncompressed message payload.
/// This is defined in [EIP-706](https://eips.ethereum.org/EIPS/eip-706).
const MAX_PAYLOAD_SIZE: usize = 16 * 1024 * 1024;

/// [`MAX_RESERVED_MESSAGE_ID`] is the maximum message ID reserved for the `p2p` subprotocol. If
/// there are any incoming messages with an ID greater than this, they are subprotocol messages.
pub const MAX_RESERVED_MESSAGE_ID: u8 = 0x0f;

/// [`MAX_P2P_MESSAGE_ID`] is the maximum message ID in use for the `p2p` subprotocol.
const MAX_P2P_MESSAGE_ID: u8 = P2PMessageID::Pong as u8;

/// [`HANDSHAKE_TIMEOUT`] determines the amount of time to wait before determining that a `p2p`
/// handshake has timed out.
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// [`PING_TIMEOUT`] determines the amount of time to wait before determining that a `p2p` ping has
/// timed out.
const PING_TIMEOUT: Duration = Duration::from_secs(15);

/// [`PING_INTERVAL`] determines the amount of time to wait between sending `p2p` ping messages
/// when the peer is responsive.
const PING_INTERVAL: Duration = Duration::from_secs(60);

/// [`MAX_P2P_CAPACITY`] is the maximum number of messages that can be buffered to be sent in the
/// `p2p` stream.
///
/// Note: this default is rather low because it is expected that the [`P2PStream`] wraps an
/// [`ECIESStream`](reth_ecies::stream::ECIESStream) which internally already buffers a few MB of
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
    pub const fn new(inner: S) -> Self {
        Self { inner }
    }

    /// Returns a reference to the inner stream.
    pub const fn inner(&self) -> &S {
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
        hello: HelloMessageWithProtocols,
    ) -> Result<(P2PStream<S>, HelloMessage), P2PStreamError> {
        trace!(?hello, "sending p2p hello to peer");

        // send our hello message with the Sink
        self.inner.send(alloy_rlp::encode(P2PMessage::Hello(hello.message())).into()).await?;

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
                if matches!(reason, DisconnectReason::TooManyPeers) {
                    // Too many peers is a very common disconnect reason that spams the DEBUG logs
                    trace!(%reason, "Disconnected by peer during handshake");
                } else {
                    debug!(%reason, "Disconnected by peer during handshake");
                };
                counter!("p2pstream.disconnected_errors").increment(1);
                Err(P2PStreamError::HandshakeError(P2PHandshakeError::Disconnected(reason)))
            }
            Err(err) => {
                debug!(%err, msg=%hex::encode(&first_message_bytes), "Failed to decode first message from peer");
                Err(P2PStreamError::HandshakeError(err.into()))
            }
            Ok(msg) => {
                debug!(?msg, "expected hello message but received another message");
                Err(P2PStreamError::HandshakeError(P2PHandshakeError::NonHelloMessageInHandshake))
            }
        }?;

        trace!(
            hello=?their_hello,
            "validating incoming p2p hello from peer"
        );

        if (hello.protocol_version as u8) != their_hello.protocol_version as u8 {
            // send a disconnect message notifying the peer of the protocol version mismatch
            self.send_disconnect(DisconnectReason::IncompatibleP2PProtocolVersion).await?;
            return Err(P2PStreamError::MismatchedProtocolVersion(GotExpected {
                got: their_hello.protocol_version,
                expected: hello.protocol_version,
            }))
        }

        // determine shared capabilities (currently returns only one capability)
        let capability_res =
            SharedCapabilities::try_new(hello.protocols, their_hello.capabilities.clone());

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
        trace!(
            %reason,
            "Sending disconnect message during the handshake",
        );
        self.inner
            .send(Bytes::from(alloy_rlp::encode(P2PMessage::Disconnect(reason))))
            .await
            .map_err(P2PStreamError::Io)
    }
}

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
///
/// This stream supports multiple shared capabilities, that were negotiated during the handshake.
///
/// ### Message-ID based multiplexing
///
/// > Each capability is given as much of the message-ID space as it needs. All such capabilities
/// > must statically specify how many message IDs they require. On connection and reception of the
/// > Hello message, both peers have equivalent information about what capabilities they share
/// > (including versions) and are able to form consensus over the composition of message ID space.
///
/// > Message IDs are assumed to be compact from ID 0x10 onwards (0x00-0x0f is reserved for the
/// > "p2p" capability) and given to each shared (equal-version, equal-name) capability in
/// > alphabetic order. Capability names are case-sensitive. Capabilities which are not shared are
/// > ignored. If multiple versions are shared of the same (equal name) capability, the numerically
/// > highest wins, others are ignored.
///
/// See also <https://github.com/ethereum/devp2p/blob/master/rlpx.md#message-id-based-multiplexing>
///
/// This stream emits _non-empty_ Bytes that start with the normalized message id, so that the first
/// byte of each message starts from 0. If this stream only supports a single capability, for
/// example `eth` then the first byte of each message will match
/// [EthMessageID](reth_eth_wire_types::message::EthMessageID).
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
    shared_capabilities: SharedCapabilities,

    /// Outgoing messages buffered for sending to the underlying stream.
    outgoing_messages: VecDeque<Bytes>,

    /// Maximum number of messages that we can buffer here before the [Sink] impl returns
    /// [`Poll::Pending`].
    outgoing_message_buffer_capacity: usize,

    /// Whether this stream is currently in the process of disconnecting by sending a disconnect
    /// message.
    disconnecting: bool,
}

impl<S> P2PStream<S> {
    /// Create a new [`P2PStream`] from the provided stream.
    /// New [`P2PStream`]s are assumed to have completed the `p2p` handshake successfully and are
    /// ready to send and receive subprotocol messages.
    pub fn new(inner: S, shared_capabilities: SharedCapabilities) -> Self {
        Self {
            inner,
            encoder: snap::raw::Encoder::new(),
            decoder: snap::raw::Decoder::new(),
            pinger: Pinger::new(PING_INTERVAL, PING_TIMEOUT),
            shared_capabilities,
            outgoing_messages: VecDeque::new(),
            outgoing_message_buffer_capacity: MAX_P2P_CAPACITY,
            disconnecting: false,
        }
    }

    /// Returns a reference to the inner stream.
    pub const fn inner(&self) -> &S {
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

    /// Returns the shared capabilities for this stream.
    ///
    /// This includes all the shared capabilities that were negotiated during the handshake and
    /// their offsets based on the number of messages of each capability.
    pub const fn shared_capabilities(&self) -> &SharedCapabilities {
        &self.shared_capabilities
    }

    /// Returns `true` if the stream has outgoing capacity.
    fn has_outgoing_capacity(&self) -> bool {
        self.outgoing_messages.len() < self.outgoing_message_buffer_capacity
    }

    /// Queues in a _snappy_ encoded [`P2PMessage::Pong`] message.
    fn send_pong(&mut self) {
        self.outgoing_messages.push_back(Bytes::from(alloy_rlp::encode(P2PMessage::Pong)));
    }

    /// Queues in a _snappy_ encoded [`P2PMessage::Ping`] message.
    pub fn send_ping(&mut self) {
        self.outgoing_messages.push_back(Bytes::from(alloy_rlp::encode(P2PMessage::Ping)));
    }
}

/// Gracefully disconnects the connection by sending a disconnect message and stop reading new
/// messages.
pub trait DisconnectP2P {
    /// Starts to gracefully disconnect.
    fn start_disconnect(&mut self, reason: DisconnectReason) -> Result<(), P2PStreamError>;

    /// Returns `true` if the connection is about to disconnect.
    fn is_disconnecting(&self) -> bool;
}

impl<S> DisconnectP2P for P2PStream<S> {
    /// Starts to gracefully disconnect the connection by sending a Disconnect message and stop
    /// reading new messages.
    ///
    /// Once disconnect process has started, the [`Stream`] will terminate immediately.
    ///
    /// # Errors
    ///
    /// Returns an error only if the message fails to compress.
    fn start_disconnect(&mut self, reason: DisconnectReason) -> Result<(), P2PStreamError> {
        // clear any buffered messages and queue in
        self.outgoing_messages.clear();
        let disconnect = P2PMessage::Disconnect(reason);
        let mut buf = Vec::with_capacity(disconnect.length());
        disconnect.encode(&mut buf);

        let mut compressed = vec![0u8; 1 + snap::raw::max_compress_len(buf.len() - 1)];
        let compressed_size =
            self.encoder.compress(&buf[1..], &mut compressed[1..]).map_err(|err| {
                debug!(
                    %err,
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

        self.outgoing_messages.push_back(compressed.into());
        self.disconnecting = true;
        Ok(())
    }

    fn is_disconnecting(&self) -> bool {
        self.disconnecting
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

            if bytes.is_empty() {
                // empty messages are not allowed
                return Poll::Ready(Some(Err(P2PStreamError::EmptyProtocolMessage)))
            }

            // first decode disconnect reasons, because they can be encoded in a variety of forms
            // over the wire, in both snappy compressed and uncompressed forms.
            //
            // see: [crate::disconnect::tests::test_decode_known_reasons]
            let id = bytes[0];
            if id == P2PMessageID::Disconnect as u8 {
                // We can't handle the error here because disconnect reasons are encoded as both:
                // * snappy compressed, AND
                // * uncompressed
                // over the network.
                //
                // If the decoding succeeds, we already checked the id and know this is a
                // disconnect message, so we can return with the reason.
                //
                // If the decoding fails, we continue, and will attempt to decode it again if the
                // message is snappy compressed. Failure handling in that step is the primary point
                // where an error is returned if the disconnect reason is malformed.
                if let Ok(reason) = DisconnectReason::decode(&mut &bytes[1..]) {
                    return Poll::Ready(Some(Err(P2PStreamError::Disconnected(reason))))
                }
            }

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
                debug!(
                    %err,
                    msg=%hex::encode(&bytes[1..]),
                    "error decompressing p2p message"
                );
                err
            })?;

            match id {
                _ if id == P2PMessageID::Ping as u8 => {
                    trace!("Received Ping, Sending Pong");
                    this.send_pong();
                    // This is required because the `Sink` may not be polled externally, and if
                    // that happens, the pong will never be sent.
                    cx.waker().wake_by_ref();
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
                _ if id == P2PMessageID::Disconnect as u8 => {
                    // At this point, the `decompress_buf` contains the snappy decompressed
                    // disconnect message.
                    //
                    // It's possible we already tried to RLP decode this, but it was snappy
                    // compressed, so we need to RLP decode it again.
                    let reason = DisconnectReason::decode(&mut &decompress_buf[1..]).inspect_err(|err| {
                        debug!(
                            %err, msg=%hex::encode(&decompress_buf[1..]), "Failed to decode disconnect message from peer"
                        );
                    })?;
                    return Poll::Ready(Some(Err(P2PStreamError::Disconnected(reason))))
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
                    decompress_buf[0] = bytes[0] - MAX_RESERVED_MESSAGE_ID - 1;

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
        if item.len() > MAX_PAYLOAD_SIZE {
            return Err(P2PStreamError::MessageTooBig {
                message_size: item.len(),
                max_size: MAX_PAYLOAD_SIZE,
            })
        }

        if item.is_empty() {
            // empty messages are not allowed
            return Err(P2PStreamError::EmptyProtocolMessage)
        }

        // ensure we have free capacity
        if !self.has_outgoing_capacity() {
            return Err(P2PStreamError::SendBufferFull)
        }

        let this = self.project();

        let mut compressed = BytesMut::zeroed(1 + snap::raw::max_compress_len(item.len() - 1));
        let compressed_size =
            this.encoder.compress(&item[1..], &mut compressed[1..]).map_err(|err| {
                debug!(
                    %err,
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
        compressed[0] = item[0] + MAX_RESERVED_MESSAGE_ID + 1;
        this.outgoing_messages.push_back(compressed.freeze());

        Ok(())
    }

    /// Returns `Poll::Ready(Ok(()))` when no buffered items remain.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let mut this = self.project();
        let poll_res = loop {
            match this.inner.as_mut().poll_ready(cx) {
                Poll::Pending => break Poll::Pending,
                Poll::Ready(Err(err)) => break Poll::Ready(Err(err.into())),
                Poll::Ready(Ok(())) => {
                    let Some(message) = this.outgoing_messages.pop_front() else {
                        break Poll::Ready(Ok(()))
                    };
                    if let Err(err) = this.inner.as_mut().start_send(message) {
                        break Poll::Ready(Err(err.into()))
                    }
                }
            }
        };

        ready!(this.inner.as_mut().poll_flush(cx))?;

        poll_res
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(self.as_mut().poll_flush(cx))?;
        ready!(self.project().inner.poll_close(cx))?;

        Poll::Ready(Ok(()))
    }
}

/// This represents only the reserved `p2p` subprotocol messages.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
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
    pub const fn message_id(&self) -> P2PMessageID {
        match self {
            Self::Hello(_) => P2PMessageID::Hello,
            Self::Disconnect(_) => P2PMessageID::Disconnect,
            Self::Ping => P2PMessageID::Ping,
            Self::Pong => P2PMessageID::Pong,
        }
    }
}

impl Encodable for P2PMessage {
    /// The [`Encodable`] implementation for [`P2PMessage::Ping`] and [`P2PMessage::Pong`] encodes
    /// the message as RLP, and prepends a snappy header to the RLP bytes for all variants except
    /// the [`P2PMessage::Hello`] variant, because the hello message is never compressed in the
    /// `p2p` subprotocol.
    fn encode(&self, out: &mut dyn BufMut) {
        (self.message_id() as u8).encode(out);
        match self {
            Self::Hello(msg) => msg.encode(out),
            Self::Disconnect(msg) => msg.encode(out),
            Self::Ping => {
                // Ping payload is _always_ snappy encoded
                out.put_u8(0x01);
                out.put_u8(0x00);
                out.put_u8(EMPTY_LIST_CODE);
            }
            Self::Pong => {
                // Pong payload is _always_ snappy encoded
                out.put_u8(0x01);
                out.put_u8(0x00);
                out.put_u8(EMPTY_LIST_CODE);
            }
        }
    }

    fn length(&self) -> usize {
        let payload_len = match self {
            Self::Hello(msg) => msg.length(),
            Self::Disconnect(msg) => msg.length(),
            // id + snappy encoded payload
            Self::Ping | Self::Pong => 3, // len([0x01, 0x00, 0xc0]) = 3
        };
        payload_len + 1 // (1 for length of p2p message id)
    }
}

impl Decodable for P2PMessage {
    /// The [`Decodable`] implementation for [`P2PMessage`] assumes that each of the message
    /// variants are snappy compressed, except for the [`P2PMessage::Hello`] variant since the
    /// hello message is never compressed in the `p2p` subprotocol.
    ///
    /// The [`Decodable`] implementation for [`P2PMessage::Ping`] and [`P2PMessage::Pong`] expects
    /// a snappy encoded payload, see [`Encodable`] implementation.
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
            P2PMessageID::Hello => Ok(Self::Hello(HelloMessage::decode(buf)?)),
            P2PMessageID::Disconnect => Ok(Self::Disconnect(DisconnectReason::decode(buf)?)),
            P2PMessageID::Ping => {
                advance_snappy_ping_pong_payload(buf)?;
                Ok(Self::Ping)
            }
            P2PMessageID::Pong => {
                advance_snappy_ping_pong_payload(buf)?;
                Ok(Self::Pong)
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
            P2PMessage::Hello(_) => Self::Hello,
            P2PMessage::Disconnect(_) => Self::Disconnect,
            P2PMessage::Ping => Self::Ping,
            P2PMessage::Pong => Self::Pong,
        }
    }
}

impl TryFrom<u8> for P2PMessageID {
    type Error = P2PStreamError;

    fn try_from(id: u8) -> Result<Self, Self::Error> {
        match id {
            0x00 => Ok(Self::Hello),
            0x01 => Ok(Self::Disconnect),
            0x02 => Ok(Self::Ping),
            0x03 => Ok(Self::Pong),
            _ => Err(P2PStreamError::UnknownReservedMessageId(id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{capability::SharedCapability, test_utils::eth_hello, EthVersion, ProtocolVersion};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_util::codec::Decoder;

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

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_can_disconnect_weird_disconnect_encoding() {
        reth_tracing::init_test_tracing();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let expected_disconnect = DisconnectReason::SubprotocolSpecific;

        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = crate::PassthroughCodec::default().framed(incoming);

            let (server_hello, _) = eth_hello();

            let (mut p2p_stream, _) =
                UnauthedP2PStream::new(stream).handshake(server_hello).await.unwrap();

            // Unrolled `disconnect` method, without compression
            p2p_stream.outgoing_messages.clear();

            p2p_stream.outgoing_messages.push_back(Bytes::from(alloy_rlp::encode(
                P2PMessage::Disconnect(DisconnectReason::SubprotocolSpecific),
            )));
            p2p_stream.disconnecting = true;
            p2p_stream.close().await.unwrap();
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

        handle.await.unwrap();
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
                *p2p_stream.shared_capabilities.iter_caps().next().unwrap(),
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
            *p2p_stream.shared_capabilities.iter_caps().next().unwrap(),
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

        let handle = tokio::spawn(Box::pin(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = crate::PassthroughCodec::default().framed(incoming);

            let (server_hello, _) = eth_hello();

            let unauthed_stream = UnauthedP2PStream::new(stream);
            match unauthed_stream.handshake(server_hello.clone()).await {
                Ok((_, hello)) => {
                    panic!("expected handshake to fail, instead got a successful Hello: {hello:?}")
                }
                Err(P2PStreamError::MismatchedProtocolVersion(GotExpected { got, expected })) => {
                    assert_ne!(expected, got);
                    assert_eq!(expected, server_hello.protocol_version);
                }
                Err(other_err) => {
                    panic!("expected mismatched protocol version error, got {other_err:?}")
                }
            }
        }));

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
            Err(P2PStreamError::MismatchedProtocolVersion(GotExpected { got, expected })) => {
                assert_ne!(expected, got);
                assert_eq!(expected, client_hello.protocol_version);
            }
            Err(other_err) => {
                panic!("expected mismatched protocol version error, got {other_err:?}")
            }
        }

        // make sure the server receives the message and asserts before ending the test
        handle.await.unwrap();
    }

    #[test]
    fn snappy_decode_encode_ping() {
        let snappy_ping = b"\x02\x01\0\xc0";
        let ping = P2PMessage::decode(&mut &snappy_ping[..]).unwrap();
        assert!(matches!(ping, P2PMessage::Ping));
        assert_eq!(alloy_rlp::encode(ping), &snappy_ping[..]);
    }

    #[test]
    fn snappy_decode_encode_pong() {
        let snappy_pong = b"\x03\x01\0\xc0";
        let pong = P2PMessage::decode(&mut &snappy_pong[..]).unwrap();
        assert!(matches!(pong, P2PMessage::Pong));
        assert_eq!(alloy_rlp::encode(pong), &snappy_pong[..]);
    }
}
