#![allow(dead_code, unreachable_pub, missing_docs, unused_variables)]
use bytes::{Buf, Bytes, BytesMut};
use futures::{ready, FutureExt, Sink, SinkExt, StreamExt};
use pin_project::pin_project;
use reth_primitives::H512 as PeerId;
use reth_rlp::{Decodable, DecodeError, Encodable, RlpDecodable, RlpEncodable};
use std::{
    fmt::Display,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio_stream::Stream;

use crate::{
    error::{P2PHandshakeError, P2PStreamError},
    pinger::{IntervalTimeoutPinger, PingerEvent},
};

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
const PING_TIMEOUT: Duration = Duration::from_secs(15);
const PING_INTERVAL: Duration = Duration::from_secs(60);
const GRACE_PERIOD: Duration = Duration::from_secs(2);
const MAX_FAILED_PINGS: u8 = 3;

/// A P2PStream wraps over any `Stream` that yields bytes and makes it compatible with `p2p`
/// protocol messages.
#[pin_project]
pub struct P2PStream<S> {
    #[pin]
    inner: S,

    /// Whether the `Hello` handshake has been completed
    authed: bool,

    /// The snappy encoder used for compressing outgoing messages
    encoder: snap::raw::Encoder,

    /// The snappy decoder used for decompressing incoming messages
    decoder: snap::raw::Decoder,

    /// The state machine used for keeping track of the peer's ping status.
    pinger: IntervalTimeoutPinger,

    // currently starts at 0x10
    // maybe we need an array of offsets and lengths for each subprotocol and can just subtract
    // based on the range of the message id / protocol length
    /// The offset? TODO: revisit when implementing shared capabilities
    offset: u8,
}

/// An un-authenticated `P2PStream`. This is consumed and returns a [`P2PStream`] after the `Hello`
/// handshake is completed.
#[pin_project]
pub struct UnauthedP2PStream<S> {
    #[pin]
    stream: P2PStream<S>,
}

impl<S> UnauthedP2PStream<S> {
    /// Create a new `UnauthedP2PStream` from a `Stream` of bytes.
    pub fn new(inner: S) -> Self {
        Self { stream: P2PStream::new(inner) }
    }
}

impl<S> UnauthedP2PStream<S>
where
    S: Stream<Item = Result<BytesMut, io::Error>> + Sink<Bytes, Error = io::Error> + Unpin,
{
    /// Consumes the `UnauthedP2PStream` and returns a `P2PStream` after the `Hello` handshake is
    /// completed.
    pub async fn handshake(mut self, hello: HelloMessage) -> Result<P2PStream<S>, P2PStreamError> {
        tracing::trace!("sending p2p hello ...");
        let mut our_hello_bytes = BytesMut::new();
        P2PMessage::Hello(hello.clone()).encode(&mut our_hello_bytes);
        println!("sending hello: {:?}", hex::encode(&our_hello_bytes));

        // debugging - raw hello encoding
        let mut raw_hello_bytes = BytesMut::new();
        hello.encode(&mut raw_hello_bytes);
        println!("raw hello: {:?}", hex::encode(&raw_hello_bytes));
        // try to decode
        let decoded_hello = HelloMessage::decode(&mut &raw_hello_bytes[..]);
        println!("decoded hello: {:?}", decoded_hello);

        self.stream.inner.send(our_hello_bytes.into()).await?;

        tracing::trace!("waiting for p2p hello from peer ...");

        // todo: fix clippy
        let hello_bytes = tokio::time::timeout(HANDSHAKE_TIMEOUT, self.stream.inner.next())
            .await
            .or(Err(P2PStreamError::HandshakeError(P2PHandshakeError::Timeout)))?
            .ok_or(P2PStreamError::HandshakeError(P2PHandshakeError::NoResponse))??;

        // let's check the compressed length first, we will need to check again once confirming
        // that it contains snappy-compressed data (this will be the case for all non-p2p messages).
        if hello_bytes.len() > MAX_PAYLOAD_SIZE {
            return Err(P2PStreamError::MessageTooBig(hello_bytes.len()))
        }

        // get the message id
        let id = *hello_bytes.first().ok_or_else(|| P2PStreamError::EmptyProtocolMessage)?;

        println!("hello message received: {:x?}", hex::encode(&hello_bytes));
        // the first message sent MUST be the hello message
        if id != P2PMessageID::Hello as u8 {
            return Err(P2PStreamError::HandshakeError(
                P2PHandshakeError::NonHelloMessageInHandshake,
            ))
        }

        // decode the hello message from the rest of the bytes
        let their_hello = HelloMessage::decode(&mut &hello_bytes[1..])?;

        // TODO: determine shared capabilities

        self.stream.authed = true;
        Ok(self.stream)
    }
}

impl<S> P2PStream<S> {
    /// Create a new unauthed [`P2PStream`] from the provided stream. You will need to manually
    /// handshake with a peer.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            authed: false,
            offset: 0x10,
            encoder: snap::raw::Encoder::new(),
            decoder: snap::raw::Decoder::new(),
            pinger: IntervalTimeoutPinger::new(MAX_FAILED_PINGS, PING_INTERVAL, PING_TIMEOUT),
        }
    }
}

/// This represents only the reserved `p2p` subprotocol messages.
#[derive(Debug, PartialEq, Eq)]
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

impl Encodable for P2PMessage {
    fn length(&self) -> usize {
        let payload_len = match self {
            P2PMessage::Hello(msg) => msg.length(),
            P2PMessage::Disconnect(msg) => msg.length(),
            P2PMessage::Ping => 3, // len([0x01, 0x00, 0x80]) = 3
            P2PMessage::Pong => 3, // len([0x01, 0x00, 0x80]) = 3
        };
        payload_len + 1 // (1 for length of p2p message id)
    }

    fn encode(&self, out: &mut dyn bytes::BufMut) {
        out.put_u8(self.message_id() as u8);
        match self {
            P2PMessage::Hello(msg) => msg.encode(out),
            P2PMessage::Disconnect(msg) => msg.encode(out),
            P2PMessage::Ping => {
                out.put_u8(0x01);
                out.put_u8(0x00);
                out.put_u8(0x80);
            },
            P2PMessage::Pong => {
                out.put_u8(0x01);
                out.put_u8(0x00);
                out.put_u8(0x80);
            },
        }
    }
}

impl Decodable for P2PMessage {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let first = buf.first().expect("cannot decode empty p2p message");
        let id = P2PMessageID::try_from(*first)
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

// S must also be `Sink` because we need to be able to respond with ping messages to follow the
// protocol
impl<S> Stream for P2PStream<S>
where
    S: Stream<Item = Result<BytesMut, io::Error>> + Sink<Bytes, Error = io::Error> + Unpin,
{
    type Item = Result<BytesMut, P2PStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        // poll the pinger to determine if we should send a ping
        let pinger_res = ready!(Pin::new(&mut this.pinger).poll_next(cx));
        match pinger_res {
            Some(Ok(PingerEvent::Ping)) => {
                // encode the ping message
                let mut ping_bytes = BytesMut::new();
                P2PMessage::Ping.encode(&mut ping_bytes);

                let send_res = Pin::new(&mut this.inner).send(ping_bytes.into()).poll_unpin(cx)?;
                ready!(send_res)
            }
            // either None (stream ended) or Some(PingEvent::Timeout) or Err(err)
            _ => {
                // encode the disconnect message
                let mut disconnect_bytes = BytesMut::new();
                P2PMessage::Disconnect(DisconnectReason::PingTimeout).encode(&mut disconnect_bytes);

                let send_res =
                    Pin::new(&mut this.inner).send(disconnect_bytes.into()).poll_unpin(cx)?;
                ready!(send_res);

                // since the ping stream has timed out, let's send a None
                return Poll::Ready(None)
            }
        };

        // we should loop here to ensure we don't return Poll::Pending if we have a message to
        // return behind any pings we need to respond to
        while let Poll::Ready(res) = this.inner.as_mut().poll_next(cx) {
            let mut bytes = match res {
                Some(Ok(bytes)) => bytes,
                Some(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
                None => return Poll::Ready(None),
            };

            let id = *bytes.first().ok_or(P2PStreamError::EmptyProtocolMessage)?;
            if id == P2PMessageID::Ping as u8 {
                // TODO: do we need to decode the ping?
                // we have received a ping, so we will send a pong
                let mut pong_bytes = BytesMut::new();
                P2PMessage::Pong.encode(&mut pong_bytes);

                let send_res = Pin::new(&mut this.inner).send(pong_bytes.into()).poll_unpin(cx)?;
                ready!(send_res)

                // continue to the next message if there is one
            } else if id == P2PMessageID::Disconnect as u8 {
                let reason = DisconnectReason::decode(&mut &bytes[1..])?;
                // TODO: do something with the reason
                return Poll::Ready(Some(Err(P2PStreamError::Disconnected)))
            } else if id == P2PMessageID::Hello as u8 {
                // we have received a hello message outside of the handshake, so we will return an
                // error
                return Poll::Ready(Some(Err(P2PStreamError::HandshakeError(
                    P2PHandshakeError::HelloNotInHandshake,
                ))))
            } else if id == P2PMessageID::Pong as u8 {
                // TODO: do we need to decode the pong?
                // if we were waiting for a pong, this will reset the pinger state
                this.pinger.pong_received()?
            } else if id > MAX_P2P_MESSAGE_ID && id <= MAX_RESERVED_MESSAGE_ID {
                // we have received an unknown reserved message
                return Poll::Ready(Some(Err(P2PStreamError::UnknownReservedMessageId(id))))
            } else {
                // we have a subprotocol message that needs to be sent in the stream.
                // first, switch the message id based on offset so the next layer can decode it
                // without being aware of the p2p stream's state (shared capabilities / the message
                // id offset)
                bytes[0] -= *this.offset;
                return Poll::Ready(Some(Ok(bytes)))
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

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_ready(cx).map_err(Into::into)
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        let id = *item.first().ok_or(P2PStreamError::EmptyProtocolMessage)?;
        if self.authed && id == P2PMessageID::Hello as u8 {
            return Err(P2PStreamError::HandshakeError(P2PHandshakeError::HelloNotInHandshake))
        }

        // TODO: we can remove this if we either accept BytesMut or if we accept something like a
        // ProtocolMessage
        let mut encoded = BytesMut::from(&item.to_vec()[..]);

        // all messages sent in this stream are subprotocol messages, so we need to switch the
        // message id based on the offset
        encoded[0] += self.offset;

        self.project().inner.start_send(encoded.into())?;
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_flush(cx).map_err(Into::into)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_close(cx).map_err(Into::into)
    }
}

// TODO: reconsider api, if we want to layer things like ethstream on top of a p2pstream, we need
// to use the above implementation. however, this might only be possible if we implement a single
// subprotocol, since ethstream is not aware of other subprotocols and their message id offsets.
// example: currently, ethstream assumes message ids start at 0x00.
// in practice this is never the case for `eth` messages over RLPx.
// can we have different implementations of Sink<EthMessage> depending on what type the underlying
// sink is?
// Sink<EthMessage> for EthStream<S> where S: Sink<Bytes, Error = io::Error> would encode the
// message and send over the inner sink
// (this would be used in a non-RLPx transport / codec)
//
// Sink<EthMessage for EthStream<S> where S: Sink<P2PMessage, Error = io::Error> would use a
// From<EthMessage> for P2PMessage impl and send that to the inner sink
// impl<S> Sink<P2PMessage> for P2PStream<S>
// where
//     S: Sink<Bytes, Error = io::Error> + Unpin,
// {
//     type Error = P2PStreamError;

//     fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
//         self.project().inner.poll_ready(cx).map_err(Into::into)
//     }

//     fn start_send(self: Pin<&mut Self>, item: P2PMessage) -> Result<(), Self::Error> {
//         if self.authed && matches!(item, P2PMessage::Hello(_)) {
//             return Err(P2PStreamError::HandshakeError(P2PHandshakeError::HelloNotInHandshake))
//         }

//         let mut bytes = BytesMut::new();
//         item.encode(&mut bytes);

//         // all messages sent in this stream are subprotocol messages, so we need to switch the
//         // message id based on the offset
//         bytes[0] += self.offset;

//         self.project().inner.start_send(bytes.into())?;
//         Ok(())
//     }

//     fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
//         self.project().inner.poll_flush(cx).map_err(Into::into)
//     }

//     fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
//         self.project().inner.poll_close(cx).map_err(Into::into)
//     }
// }

/// A message indicating a supported capability and capability version.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct CapabilityMessage {
    /// The name of the subprotocol
    pub name: String,
    /// The version of the subprotocol
    pub version: usize,
}

impl CapabilityMessage {
    /// Create a new `CapabilityMessage` with the given name and version.
    pub fn new(name: String, version: usize) -> Self {
        Self { name, version }
    }
}

// TODO: determine if we should allow for the extra fields at the end like EIP-706 suggests
/// Message used in the `p2p` handshake, containing information about the supported RLPx protocol
/// version and capabilities.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct HelloMessage {
    /// The version of the `p2p` protocol.
    pub protocol_version: ProtocolVersion,
    /// Specifies the client software identity, as a human-readable string (e.g.
    /// "Ethereum(++)/1.0.0").
    pub client_version: String,
    /// The list of supported capabilities and their versions.
    pub capabilities: Vec<CapabilityMessage>,
    /// The port that the client is listening on, zero indicates the client is not listening.
    pub port: u16,
    /// The secp256k1 public key corresponding to the node's private key.
    pub id: PeerId,
}

/// RLPx `p2p` protocol version
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ProtocolVersion {
    /// `p2p` version 4
    V4 = 4,
    /// `p2p` version 5
    V5 = 5,
}

impl Encodable for ProtocolVersion {
    fn length(&self) -> usize {
        // the version should be a single byte
        (*self as u8).length()
    }
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        (*self as u8).encode(out)
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

/// RLPx disconnect reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectReason {
    /// Disconnect requested by the local node or remote peer.
    DisconnectRequested = 0x00,
    /// TCP related error
    TcpSubsystemError = 0x01,
    /// Breach of protocol at the transport or p2p level
    ProtocolBreach = 0x02,
    /// Node has no matching protocols.
    UselessPeer = 0x03,
    /// Either the remote or local node has too many peers.
    TooManyPeers = 0x04,
    /// Already connected to the peer.
    AlreadyConnected = 0x05,
    /// `p2p` protocol version is incompatible
    IncompatibleP2PProtocolVersion = 0x06,
    NullNodeIdentity = 0x07,
    ClientQuitting = 0x08,
    UnexpectedHandshakeIdentity = 0x09,
    /// The node is connected to itself
    ConnectedToSelf = 0x0a,
    /// Peer or local node did not respond to a ping in time.
    PingTimeout = 0x0b,
    /// Peer or local node violated a subprotocol-specific rule.
    SubprotocolSpecific = 0x10,
}

impl Display for DisconnectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            DisconnectReason::DisconnectRequested => "Disconnect requested",
            DisconnectReason::TcpSubsystemError => "TCP sub-system error",
            DisconnectReason::ProtocolBreach => {
                "Breach of protocol, e.g. a malformed message, bad RLP, ..."
            }
            DisconnectReason::UselessPeer => "Useless peer",
            DisconnectReason::TooManyPeers => "Too many peers",
            DisconnectReason::AlreadyConnected => "Already connected",
            DisconnectReason::IncompatibleP2PProtocolVersion => "Incompatible P2P protocol version",
            DisconnectReason::NullNodeIdentity => {
                "Null node identity received - this is automatically invalid"
            }
            DisconnectReason::ClientQuitting => "Client quitting",
            DisconnectReason::UnexpectedHandshakeIdentity => "Unexpected identity in handshake",
            DisconnectReason::ConnectedToSelf => {
                "Identity is the same as this node (i.e. connected to itself)"
            }
            DisconnectReason::PingTimeout => "Ping timeout",
            DisconnectReason::SubprotocolSpecific => "Some other reason specific to a subprotocol",
        };

        write!(f, "{}", message)
    }
}

/// This represents an unknown disconnect reason with the given code.
#[derive(Debug, Clone)]
pub struct UnknownDisconnectReason(u8);

impl TryFrom<u8> for DisconnectReason {
    // This error type should not be used to crash the node, but rather to log the error and
    // disconnect the peer.
    type Error = UnknownDisconnectReason;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(DisconnectReason::DisconnectRequested),
            0x01 => Ok(DisconnectReason::TcpSubsystemError),
            0x02 => Ok(DisconnectReason::ProtocolBreach),
            0x03 => Ok(DisconnectReason::UselessPeer),
            0x04 => Ok(DisconnectReason::TooManyPeers),
            0x05 => Ok(DisconnectReason::AlreadyConnected),
            0x06 => Ok(DisconnectReason::IncompatibleP2PProtocolVersion),
            0x07 => Ok(DisconnectReason::NullNodeIdentity),
            0x08 => Ok(DisconnectReason::ClientQuitting),
            0x09 => Ok(DisconnectReason::UnexpectedHandshakeIdentity),
            0x0a => Ok(DisconnectReason::ConnectedToSelf),
            0x0b => Ok(DisconnectReason::PingTimeout),
            0x10 => Ok(DisconnectReason::SubprotocolSpecific),
            _ => Err(UnknownDisconnectReason(value)),
        }
    }
}

impl Encodable for DisconnectReason {
    fn length(&self) -> usize {
        // disconnect reasons are snappy encoded as follows:
        // [0x01, 0x00, reason as u8]
        // this is 3 bytes
        3
    }
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        // disconnect reasons are snappy encoded as follows:
        // [0x01, 0x00, reason as u8]
        // this is 3 bytes
        out.put_u8(0x01);
        out.put_u8(0x00);
        out.put_u8(*self as u8);
    }
}

impl Decodable for DisconnectReason {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let first = *buf.first().expect("disconnect reason should have at least 1 byte");
        buf.advance(1);
        if first != 0x01 {
            return Err(DecodeError::Custom("invalid disconnect reason - invalid snappy header"));
        }

        let second = *buf.first().expect("disconnect reason should have at least 2 bytes");
        buf.advance(1);
        if second != 0x00 {
            // TODO: make sure this error message is correct
            return Err(DecodeError::Custom("invalid disconnect reason - invalid snappy header"));
        }

        let reason = *buf.first().expect("disconnect reason should have 3 bytes");
        buf.advance(1);
        DisconnectReason::try_from(reason)
            .map_err(|_| DecodeError::Custom("unknown disconnect reason"))
    }
}

// TODO: test ping pong stuff
#[cfg(test)]
mod tests {
    use reth_ecies::util::pk2id;
    use reth_rlp::EMPTY_STRING_CODE;
    use secp256k1::{SecretKey, SECP256K1};

    use crate::EthVersion;

    use super::*;

    #[tokio::test]
    async fn test_handshake_passthrough() {
        // TODO
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
        assert_eq!(ping_expected, ping_encoded, "left: {:#x?}, right: {:#x?}", ping_expected, ping_encoded);

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

    #[test]
    fn test_pong_snappy_encoding_parity() {
        // encode pong using our `Encodable` implementation
        let pong = P2PMessage::Pong;
        let mut pong_encoded = Vec::new();
        pong.encode(&mut pong_encoded);

        // the definition of pong is 0x80 (an empty rlp string)
        let pong_raw = vec![EMPTY_STRING_CODE];
        let mut snappy_encoder = snap::raw::Encoder::new();
        let pong_compressed = snappy_encoder.compress_vec(&pong_raw).unwrap();
        let mut pong_expected = vec![P2PMessageID::Pong as u8];
        pong_expected.extend(&pong_compressed);

        // ensure that the two encodings are equal
        assert_eq!(pong_expected, pong_encoded, "left: {:#x?}, right: {:#x?}", pong_expected, pong_encoded);

        // also ensure that the length is correct
        assert_eq!(pong_expected.len(), P2PMessage::Pong.length());

        // try to decode using Decodable
        let p2p_message = P2PMessage::decode(&mut &pong_expected[..]).unwrap();
        assert_eq!(p2p_message, P2PMessage::Pong);

        // finally decode the encoded message with snappy
        let mut snappy_decoder = snap::raw::Decoder::new();

        // the message id is not compressed, only compress the latest bits
        let decompressed = snappy_decoder.decompress_vec(&pong_encoded[1..]).unwrap();

        assert_eq!(decompressed, pong_raw);
    }

    #[test]
    fn test_hello_encoding_round_trip() {
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let id = pk2id(&secret_key.public_key(SECP256K1));
        let hello = P2PMessage::Hello(HelloMessage {
            protocol_version: ProtocolVersion::V5,
            client_version: "reth/0.1.0".to_string(),
            capabilities: vec![CapabilityMessage::new(
                "eth".to_string(),
                EthVersion::Eth67 as usize,
            )],
            port: 30303,
            id,
        });

        let mut hello_encoded = Vec::new();
        hello.encode(&mut hello_encoded);

        let hello_decoded = P2PMessage::decode(&mut &hello_encoded[..]).unwrap();

        assert_eq!(hello, hello_decoded);
    }

    #[test]
    fn hello_encoding_length() {
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let id = pk2id(&secret_key.public_key(SECP256K1));
        let hello = P2PMessage::Hello(HelloMessage {
            protocol_version: ProtocolVersion::V5,
            client_version: "reth/0.1.0".to_string(),
            capabilities: vec![CapabilityMessage::new(
                "eth".to_string(),
                EthVersion::Eth67 as usize,
            )],
            port: 30303,
            id,
        });

        let mut hello_encoded = Vec::new();
        hello.encode(&mut hello_encoded);

        assert_eq!(hello_encoded.len(), hello.length());
    }

    #[test]
    fn disconnect_round_trip() {
        let all_reasons = vec![
            DisconnectReason::DisconnectRequested,
            DisconnectReason::TcpSubsystemError,
            DisconnectReason::ProtocolBreach,
            DisconnectReason::UselessPeer,
            DisconnectReason::TooManyPeers,
            DisconnectReason::AlreadyConnected,
            DisconnectReason::IncompatibleP2PProtocolVersion,
            DisconnectReason::NullNodeIdentity,
            DisconnectReason::ClientQuitting,
            DisconnectReason::UnexpectedHandshakeIdentity,
            DisconnectReason::ConnectedToSelf,
            DisconnectReason::PingTimeout,
            DisconnectReason::SubprotocolSpecific,
        ];

        for reason in all_reasons {
            let disconnect = P2PMessage::Disconnect(reason);

            let mut disconnect_encoded = Vec::new();
            disconnect.encode(&mut disconnect_encoded);

            let disconnect_decoded = P2PMessage::decode(&mut &disconnect_encoded[..]).unwrap();

            assert_eq!(disconnect, disconnect_decoded);
        }
    }

    #[test]
    fn disconnect_encoding_length() {
        let all_reasons = vec![
            DisconnectReason::DisconnectRequested,
            DisconnectReason::TcpSubsystemError,
            DisconnectReason::ProtocolBreach,
            DisconnectReason::UselessPeer,
            DisconnectReason::TooManyPeers,
            DisconnectReason::AlreadyConnected,
            DisconnectReason::IncompatibleP2PProtocolVersion,
            DisconnectReason::NullNodeIdentity,
            DisconnectReason::ClientQuitting,
            DisconnectReason::UnexpectedHandshakeIdentity,
            DisconnectReason::ConnectedToSelf,
            DisconnectReason::PingTimeout,
            DisconnectReason::SubprotocolSpecific,
        ];

        for reason in all_reasons {
            let disconnect = P2PMessage::Disconnect(reason);

            let mut disconnect_encoded = Vec::new();
            disconnect.encode(&mut disconnect_encoded);

            assert_eq!(disconnect_encoded.len(), disconnect.length());
        }
    }
}
