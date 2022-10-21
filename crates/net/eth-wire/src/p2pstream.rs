use bytes::{Bytes, BytesMut};
use futures::{Sink, StreamExt};
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

use crate::error::{P2PHandshakeError, P2PStreamError};

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
const MAX_FAILED_PINGS: usize = 3;

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
    // shared capabilities: todo, needed for message id conversion
}

impl<S> P2PStream<S> {
    /// Create a new unauthed [`P2PStream`] from the provided stream. You will need to manually
    /// handshake with a peer.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            authed: false,
            encoder: snap::raw::Encoder::new(),
            decoder: snap::raw::Decoder::new(),
        }
    }
}

impl<S> P2PStream<S>
where
    S: Stream<Item = Result<BytesMut, io::Error>> + Sink<Bytes, Error = io::Error> + Unpin,
{
    /// Given an instantiated transport layer, it proceeds to return a [`P2PStream`]
    /// after performing a [`Hello`] message handshake.
    pub async fn connect(inner: S, hello: HelloMessage) -> Result<Self, P2PStreamError> {
        let mut this = Self::new(inner);
        this.handshake(hello).await?;
        Ok(this)
    }

    /// Perform a handshake with the peer. This will send a [`Hello`] message and wait for a
    /// [`Hello`] message in response.
    // TODO: do we need to return a task here? and store the task handle in the P2PStream in
    // `connect`? then recognize pings / pongs / disconnects and send them through a channel to the
    // task?
    pub async fn handshake(&mut self, hello: HelloMessage) -> Result<(), P2PStreamError> {
        tracing::trace!("sending p2p hello ...");
        // TODO: fix
        // self.send(hello.into()).await?;

        tracing::trace!("waiting for p2p hello from peer ...");

        // todo: fix clippy
        let hello_bytes = tokio::time::timeout(HANDSHAKE_TIMEOUT, self.next())
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

        // the first message sent MUST be the hello message
        if id != P2PMessageID::Hello as u8 {
            return Err(P2PStreamError::HandshakeError(
                P2PHandshakeError::NonHelloMessageInHandshake,
            ))
        }

        // decode the hello message from the rest of the bytes
        let their_hello = HelloMessage::decode(&mut &hello_bytes[1..])?;

        // todo: determine shared capabilities

        self.authed = true;
        Ok(())
    }
}

/// This represents only the reserved `p2p` subprotocol messages.
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

// TODO: determine what snappy::encode(disconnectreason) comes out to
// TODO: determine what snappy::encode(ping) and snappy::encode(pong) come out to (they both encode
// 0x80 i think)
// ANSWER:
// snappy::encode(ping): [0x01, 0x00, 0x80]
// snappy::encode(pong): [0x01, 0x00, 0x80]
// snappy::encode(reason): [0x01, 0x00, reason as u8]
// the reason for doing this is so we don't need to run it through the encoder or decoder.

// EIP-706 mentions allowing for some additional length in the hello message.
// Should we really follow this? Is a RLPx Devp2p protocol upgrade more or less likely than a
// complete overhaul of the p2p protocol?
// should we even accept protocol v4?

// how should we accept arbitrary subprotocol messages?
// we need to transform the message id depending on the shared capabilities - we should do this
// with some easy methods in the p2pstream struct

impl<S> Stream for P2PStream<S>
where
    S: Stream<Item = Result<BytesMut, io::Error>> + Unpin,
{
    type Item = Result<BytesMut, P2PStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // we should use BytesMut
        todo!()
    }
}

impl<S> Sink<P2PMessage> for P2PStream<S>
where
    S: Sink<Bytes, Error = io::Error> + Unpin,
{
    type Error = P2PStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        todo!()
    }

    fn start_send(self: Pin<&mut Self>, item: P2PMessage) -> Result<(), Self::Error> {
        todo!()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        todo!()
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        todo!()
    }
}

/// A message indicating a supported capability and capability version.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct CapabilityMessage {
    /// The name of the subprotocol
    pub name: String,
    /// The version of the subprotocol
    pub version: usize,
}

// TODO: determine if we should allow for the extra fields at the end like EIP-706 suggests
/// Message used in the `p2p` handshake, containing information about the supported RLPx protocol
/// version and capabilities.
#[derive(Clone, Debug, RlpEncodable, RlpDecodable)]
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
#[derive(Copy, Clone, Debug)]
pub enum ProtocolVersion {
    /// `p2p` version 4
    V4 = 4,
    /// `p2p` version 5
    V5 = 5,
}

impl Encodable for ProtocolVersion {
    fn length(&self) -> usize {
        // the version should be a single byte
        1
    }
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        out.put_u8(*self as u8);
    }
}

impl Decodable for ProtocolVersion {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let version = buf.first().ok_or(DecodeError::Custom("protocol version cannot be empty"))?;
        match version {
            4 => Ok(ProtocolVersion::V4),
            5 => Ok(ProtocolVersion::V5),
            _ => Err(DecodeError::Custom("unknown p2p protocol version")),
        }
    }
}

/// RLPx disconnect reason.
#[derive(Clone, Copy, Debug)]
pub enum DisconnectReason {
    /// Disconnect requested by the local node or remote peer.
    DisconnectRequested = 0x00,
    /// TCP related error
    TcpSubsystemError = 0x01,
    /// Breach of protocol at the transport or p2p level
    ProtocolBreach = 0x02,
    /// Useless peer
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
