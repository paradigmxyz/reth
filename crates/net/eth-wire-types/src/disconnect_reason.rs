//! `RLPx` disconnect reason sent to/received from peer

use alloy_primitives::bytes::{Buf, BufMut};
use alloy_rlp::{Decodable, Encodable, Header};
use derive_more::Display;
use reth_codecs_derive::add_arbitrary_tests;
use thiserror::Error;

/// RLPx disconnect reason.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Display)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub enum DisconnectReason {
    /// Disconnect requested by the local node or remote peer.
    #[default]
    #[display("disconnect requested")]
    DisconnectRequested = 0x00,
    /// TCP related error
    #[display("TCP sub-system error")]
    TcpSubsystemError = 0x01,
    /// Breach of protocol at the transport or p2p level
    #[display("breach of protocol, e.g. a malformed message, bad RLP, etc.")]
    ProtocolBreach = 0x02,
    /// Node has no matching protocols.
    #[display("useless peer")]
    UselessPeer = 0x03,
    /// Either the remote or local node has too many peers.
    #[display("too many peers")]
    TooManyPeers = 0x04,
    /// Already connected to the peer.
    #[display("already connected")]
    AlreadyConnected = 0x05,
    /// `p2p` protocol version is incompatible
    #[display("incompatible P2P protocol version")]
    IncompatibleP2PProtocolVersion = 0x06,
    /// Received a null node identity.
    #[display("null node identity received - this is automatically invalid")]
    NullNodeIdentity = 0x07,
    /// Reason when the client is shutting down.
    #[display("client quitting")]
    ClientQuitting = 0x08,
    /// When the received handshake's identify is different from what is expected.
    #[display("unexpected identity in handshake")]
    UnexpectedHandshakeIdentity = 0x09,
    /// The node is connected to itself
    #[display("identity is the same as this node (i.e. connected to itself)")]
    ConnectedToSelf = 0x0a,
    /// Peer or local node did not respond to a ping in time.
    #[display("ping timeout")]
    PingTimeout = 0x0b,
    /// Peer or local node violated a subprotocol-specific rule.
    #[display("some other reason specific to a subprotocol")]
    SubprotocolSpecific = 0x10,
}

impl TryFrom<u8> for DisconnectReason {
    // This error type should not be used to crash the node, but rather to log the error and
    // disconnect the peer.
    type Error = UnknownDisconnectReason;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::DisconnectRequested),
            0x01 => Ok(Self::TcpSubsystemError),
            0x02 => Ok(Self::ProtocolBreach),
            0x03 => Ok(Self::UselessPeer),
            0x04 => Ok(Self::TooManyPeers),
            0x05 => Ok(Self::AlreadyConnected),
            0x06 => Ok(Self::IncompatibleP2PProtocolVersion),
            0x07 => Ok(Self::NullNodeIdentity),
            0x08 => Ok(Self::ClientQuitting),
            0x09 => Ok(Self::UnexpectedHandshakeIdentity),
            0x0a => Ok(Self::ConnectedToSelf),
            0x0b => Ok(Self::PingTimeout),
            0x10 => Ok(Self::SubprotocolSpecific),
            _ => Err(UnknownDisconnectReason(value)),
        }
    }
}

impl Encodable for DisconnectReason {
    /// The [`Encodable`] implementation for [`DisconnectReason`] encodes the disconnect reason in
    /// a single-element RLP list.
    fn encode(&self, out: &mut dyn BufMut) {
        vec![*self as u8].encode(out);
    }
    fn length(&self) -> usize {
        vec![*self as u8].length()
    }
}

impl Decodable for DisconnectReason {
    /// The [`Decodable`] implementation for [`DisconnectReason`] supports either a disconnect
    /// reason encoded a single byte or a RLP list containing the disconnect reason.
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        if buf.is_empty() {
            return Err(alloy_rlp::Error::InputTooShort)
        } else if buf.len() > 2 {
            return Err(alloy_rlp::Error::Overflow)
        }

        if buf.len() > 1 {
            // this should be a list, so decode the list header. this should advance the buffer so
            // buf[0] is the first (and only) element of the list.
            let header = Header::decode(buf)?;

            if !header.list {
                return Err(alloy_rlp::Error::UnexpectedString)
            }

            if header.payload_length != 1 {
                return Err(alloy_rlp::Error::ListLengthMismatch {
                    expected: 1,
                    got: header.payload_length,
                })
            }
        }

        // geth rlp encodes [`DisconnectReason::DisconnectRequested`] as 0x00 and not as empty
        // string 0x80
        if buf[0] == 0x00 {
            buf.advance(1);
            Ok(Self::DisconnectRequested)
        } else {
            Self::try_from(u8::decode(buf)?)
                .map_err(|_| alloy_rlp::Error::Custom("unknown disconnect reason"))
        }
    }
}

/// This represents an unknown disconnect reason with the given code.
#[derive(Debug, Clone, Error)]
#[error("unknown disconnect reason: {0}")]
pub struct UnknownDisconnectReason(u8);
