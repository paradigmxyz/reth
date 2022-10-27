#![allow(dead_code, unreachable_pub, missing_docs, unused_variables)]
use bytes::{Buf, Bytes, BytesMut};
use futures::{ready, Future, Sink, SinkExt, StreamExt};
use pin_project::pin_project;
use pin_utils::pin_mut;
use reth_primitives::H512 as PeerId;
use reth_rlp::{Decodable, DecodeError, Encodable, RlpDecodable, RlpEncodable};
use std::{
    fmt::Display,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use thiserror::Error;
use tokio::time::{Instant, Sleep, Timeout};
use tokio_stream::Stream;

use crate::error::{P2PHandshakeError, P2PStreamError, PingerError, TimeoutPingerError};

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
    pinger: Pinger,

    // currently starts at 0x10
    // maybe we need an array of offsets and lengths for each subprotocol and can just subtract
    // based on the range of the message id / protocol length
    /// The offset? TODO: revisit when implementing shared capabilities
    offset: u8,
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
            pinger: Pinger::new(MAX_FAILED_PINGS),
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
    pub async fn handshake(&mut self, hello: HelloMessage) -> Result<(), P2PStreamError> {
        tracing::trace!("sending p2p hello ...");
        let mut hello_bytes = BytesMut::new();
        P2PMessage::Hello(hello).encode(&mut hello_bytes);
        self.inner.send(hello_bytes.into()).await?;

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

        // TODO: determine shared capabilities

        self.authed = true;
        Ok(())
    }
}

/// This represents the possible states of the pinger.
pub enum PingState {
    /// There are no pings in flight, or all pings have been responded to and we are ready to send
    /// a ping at a later point.
    Ready,

    /// We have sent a ping and are waiting for a pong, but the peer has missed n pongs.
    WaitingForPong(u8),

    /// The peer has missed n pongs and is considered timed out.
    TimedOut(u8),
}

/// The pinger is a state machine that is created with a maximum number of pongs that can be
/// missed.
pub struct Pinger {
    /// The maximum number of pongs that can be missed.
    max_missed: u8,

    /// The current state of the pinger.
    state: PingState,
}

impl Pinger {
    /// Create a new pinger with the given maximum number of pongs that can be missed.
    pub fn new(max_missed: u8) -> Self {
        Self { max_missed, state: PingState::Ready }
    }

    /// Return the current state of the pinger.
    pub fn state(&self) -> &PingState {
        &self.state
    }

    /// Check if the pinger is in the `Ready` state.
    pub fn is_ready(&self) -> bool {
        matches!(self.state, PingState::Ready)
    }

    /// Check if the pinger is in the `WaitingForPong` state.
    pub fn is_waiting_for_pong(&self) -> bool {
        matches!(self.state, PingState::WaitingForPong(_))
    }

    /// Check if the pinger is in the `TimedOut` state.
    pub fn is_timed_out(&self) -> bool {
        matches!(self.state, PingState::TimedOut(_))
    }

    /// Mark a ping as sent, and transition the pinger to the `WaitingForPong` state if it was in
    /// the `Ready` state.
    ///
    /// If the pinger is in the `WaitingForPong` state, the number of missed pongs will be
    /// incremented. If the number of missed pongs exceeds the maximum missed pongs allowed, the
    /// pinger will be transitioned to the `TimedOut` state.
    ///
    /// If the pinger is in the `TimedOut` state, this method will return an error.
    pub fn ping_sent(&mut self) -> Result<(), PingerError> {
        match self.state {
            PingState::Ready => {
                self.state = PingState::WaitingForPong(0);
                Ok(())
            }
            PingState::WaitingForPong(missed) => {
                if missed + 1 >= self.max_missed {
                    self.state = PingState::TimedOut(missed + 1);
                    Ok(())
                } else {
                    self.state = PingState::WaitingForPong(missed + 1);
                    Ok(())
                }
            }
            PingState::TimedOut(_) => Err(PingerError::PingWhileTimedOut),
        }
    }

    /// Mark a pong as received, and transition the pinger to the `Ready` state if it was in the
    /// `WaitingForPong` state.
    ///
    /// If the pinger is in the `Ready` or `TimedOut` state, this method will return an error.
    pub fn pong_received(&mut self) -> Result<(), PingerError> {
        match self.state {
            PingState::Ready => Err(PingerError::PongWhileReady),
            PingState::WaitingForPong(_) => {
                self.state = PingState::Ready;
                Ok(())
            }
            PingState::TimedOut(_) => Err(PingerError::PongWhileTimedOut),
        }
    }
}

// TODO: pinger poll

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

// TODO: snappy stuff for non-hello
impl Encodable for P2PMessage {
    fn length(&self) -> usize {
        match self {
            P2PMessage::Hello(msg) => msg.length(),
            P2PMessage::Disconnect(msg) => msg.length(),
            P2PMessage::Ping => 1,
            P2PMessage::Pong => 1,
        }
    }

    fn encode(&self, out: &mut dyn bytes::BufMut) {
        match self {
            P2PMessage::Hello(msg) => msg.encode(out),
            P2PMessage::Disconnect(msg) => msg.encode(out),
            P2PMessage::Ping => out.put_u8(P2PMessageID::Ping as u8),
            P2PMessage::Pong => out.put_u8(P2PMessageID::Pong as u8),
        }
    }
}

// TODO: snappy stuff for non-hello
impl Decodable for P2PMessage {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let first = buf.first().expect("cannot decode empty p2p message");
        let id = P2PMessageID::try_from(*first)
            .or(Err(DecodeError::Custom("unknown p2p message id")))?;
        match id {
            P2PMessageID::Hello => Ok(P2PMessage::Hello(HelloMessage::decode(buf)?)),
            P2PMessageID::Disconnect => Ok(P2PMessage::Disconnect(DisconnectReason::decode(buf)?)),
            P2PMessageID::Ping => {
                buf.advance(1);
                Ok(P2PMessage::Ping)
            }
            P2PMessageID::Pong => {
                buf.advance(1);
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

// snappy encodings of ping/pong/discreason:
// snappy::encode(ping): [0x01, 0x00, 0x80]
// snappy::encode(pong): [0x01, 0x00, 0x80]
// snappy::encode(reason): [0x01, 0x00, reason as u8]
// TODOs referring to snappy are for encoding/decoding these messages

// EIP-706 mentions allowing for some additional length in the hello message.
// Should we really follow this? Is a RLPx Devp2p protocol upgrade more or less likely than a
// complete overhaul of the p2p protocol?
// should we even accept protocol v4?

// S must also be `Sink` because we need to be able to respond with ping messages to follow the
// protocol
impl<S> Stream for P2PStream<S>
where
    S: Stream<Item = Result<BytesMut, io::Error>> + Sink<Bytes, Error = io::Error> + Unpin,
{
    type Item = Result<BytesMut, P2PStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

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
                // we have received a ping, so we will send a pong
                let mut pong_bytes = BytesMut::new();
                P2PMessage::Pong.encode(&mut pong_bytes);

                // make sure the sink is ready before we send the pong
                match this.inner.as_mut().poll_ready(cx) {
                    Poll::Ready(Ok(())) => {}
                    Poll::Ready(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
                    Poll::Pending => return Poll::Pending,
                };

                // send the pong
                match this.inner.as_mut().start_send(pong_bytes.into()) {
                    Ok(()) => {}
                    Err(err) => return Poll::Ready(Some(Err(err.into()))),
                };

                // flush the sink
                match this.inner.as_mut().poll_flush(cx) {
                    Poll::Ready(Ok(())) => {}
                    Poll::Ready(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
                    Poll::Pending => return Poll::Pending,
                };

                // continue to the next message if there is one
                continue
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
                // we have received a pong without waiting for a pong
                return Poll::Ready(Some(Err(PingerError::PongWhileReady.into())))
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

impl<S> Sink<P2PMessage> for P2PStream<S>
where
    S: Sink<Bytes, Error = io::Error> + Unpin,
{
    type Error = P2PStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_ready(cx).map_err(Into::into)
    }

    fn start_send(self: Pin<&mut Self>, item: P2PMessage) -> Result<(), Self::Error> {
        if self.authed && matches!(item, P2PMessage::Hello(_)) {
            return Err(P2PStreamError::HandshakeError(P2PHandshakeError::HelloNotInHandshake))
        }

        let mut bytes = BytesMut::new();
        item.encode(&mut bytes);

        // all messages sent in this stream are subprotocol messages, so we need to switch the
        // message id based on the offset
        bytes[0] += self.offset;

        self.project().inner.start_send(bytes.into())?;
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_flush(cx).map_err(Into::into)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_close(cx).map_err(Into::into)
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
        // the reason should be a single byte
        1
    }
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        out.put_u8(*self as u8);
        // TODO: switch to snappy format
    }
}

// TODO: switch to snappy format
impl Decodable for DisconnectReason {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let reason = buf.first().ok_or(DecodeError::Custom("disconnect reason cannot be empty"))?;
        DisconnectReason::try_from(*reason)
            .map_err(|_| DecodeError::Custom("unknown disconnect reason"))
    }
}

// TODO: test ping pong stuff
#[cfg(test)]
mod tests {
    use reth_ecies::{stream::ECIESStream, util::pk2id};
    use secp256k1::{SecretKey, SECP256K1};
    use tokio::net::{TcpListener, TcpStream};

    use crate::{BlockHashNumber, EthMessage, EthStream};

    use super::*;

    #[tokio::test]
    async fn ping_pong_test() {
        // create a p2p stream and server, then confirm that the two are authed
        // create tcpstream
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let server_key = SecretKey::new(&mut rand::thread_rng());
        let test_msg = EthMessage::NewBlockHashes(
            vec![
                BlockHashNumber { hash: reth_primitives::H256::random(), number: 5 },
                BlockHashNumber { hash: reth_primitives::H256::random(), number: 6 },
            ]
            .into(),
        );

        let test_msg_clone = test_msg.clone();
        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = ECIESStream::incoming(incoming, server_key).await.unwrap();

            // TODO: p2p stream replacement
            let mut p2p_stream = P2PStream::new(stream);

            // use the stream to get the next message
            let message = stream.next().await.unwrap().unwrap();
            assert_eq!(message, test_msg_clone);
        });

        // create the server pubkey
        let server_id = pk2id(&server_key.public_key(SECP256K1));

        let client_key = SecretKey::new(&mut rand::thread_rng());

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let outgoing = ECIESStream::connect(outgoing, client_key, server_id).await.unwrap();
        let mut client_stream = P2PStream::new(outgoing);

        client_stream.send(test_msg).await.unwrap();

        // make sure the server receives the message and asserts before ending the test
        handle.await.unwrap();
    }
}
