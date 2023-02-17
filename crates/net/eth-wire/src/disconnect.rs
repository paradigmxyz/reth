//! Disconnect

use reth_codecs::derive_arbitrary;
use reth_primitives::bytes::{Buf, BufMut};
use reth_rlp::{Decodable, DecodeError, Encodable, Header};
use std::fmt::Display;
use thiserror::Error;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// RLPx disconnect reason.
#[derive_arbitrary(rlp)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum DisconnectReason {
    /// Disconnect requested by the local node or remote peer.
    #[default]
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
    /// Received a null node identity.
    NullNodeIdentity = 0x07,
    /// Reason when the client is shutting down.
    ClientQuitting = 0x08,
    /// When the received handshake's identify is different from what is expected.
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

        write!(f, "{message}")
    }
}

/// This represents an unknown disconnect reason with the given code.
#[derive(Debug, Clone, Error)]
#[error("unknown disconnect reason: {0}")]
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

/// The [`Encodable`](reth_rlp::Encodable) implementation for [`DisconnectReason`] encodes the
/// disconnect reason in a single-element RLP list.
impl Encodable for DisconnectReason {
    fn encode(&self, out: &mut dyn BufMut) {
        vec![*self as u8].encode(out);
    }
    fn length(&self) -> usize {
        vec![*self as u8].length()
    }
}

/// The [`Decodable`](reth_rlp::Decodable) implementation for [`DisconnectReason`] supports either
/// a disconnect reason encoded a single byte or a RLP list containing the disconnect reason.
impl Decodable for DisconnectReason {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        if buf.is_empty() {
            return Err(DecodeError::InputTooShort)
        } else if buf.len() > 2 {
            return Err(DecodeError::Overflow)
        }

        if buf.len() > 1 {
            // this should be a list, so decode the list header. this should advance the buffer so
            // buf[0] is the first (and only) element of the list.
            let header = Header::decode(buf)?;
            if !header.list {
                return Err(DecodeError::UnexpectedString)
            }
        }

        // geth rlp encodes [`DisconnectReason::DisconnectRequested`] as 0x00 and not as empty
        // string 0x80
        if buf[0] == 0x00 {
            buf.advance(1);
            Ok(DisconnectReason::DisconnectRequested)
        } else {
            DisconnectReason::try_from(u8::decode(buf)?)
                .map_err(|_| DecodeError::Custom("unknown disconnect reason"))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{p2pstream::P2PMessage, DisconnectReason};
    use reth_primitives::hex;
    use reth_rlp::{Decodable, Encodable};

    fn all_reasons() -> Vec<DisconnectReason> {
        vec![
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
        ]
    }

    #[test]
    fn disconnect_round_trip() {
        let all_reasons = all_reasons();

        for reason in all_reasons {
            let disconnect = P2PMessage::Disconnect(reason);

            let mut disconnect_encoded = Vec::new();
            disconnect.encode(&mut disconnect_encoded);

            let disconnect_decoded = P2PMessage::decode(&mut &disconnect_encoded[..]).unwrap();

            assert_eq!(disconnect, disconnect_decoded);
        }
    }

    #[test]
    fn test_reason_too_short() {
        assert!(DisconnectReason::decode(&mut &[0u8; 0][..]).is_err())
    }

    #[test]
    fn test_reason_too_long() {
        assert!(DisconnectReason::decode(&mut &[0u8; 3][..]).is_err())
    }

    #[test]
    fn disconnect_encoding_length() {
        let all_reasons = all_reasons();

        for reason in all_reasons {
            let disconnect = P2PMessage::Disconnect(reason);

            let mut disconnect_encoded = Vec::new();
            disconnect.encode(&mut disconnect_encoded);

            assert_eq!(disconnect_encoded.len(), disconnect.length());
        }
    }

    #[test]
    fn test_decode_known_reasons() {
        let all_reasons = vec![
            // encoding the disconnect reason as a single byte
            "0100", // 0x00 case
            "0180", // second 0x00 case
            "0101", "0102", "0103", "0104", "0105", "0106", "0107", "0108", "0109", "010a", "010b",
            "0110",   // encoding the disconnect reason in a list
            "01c100", // 0x00 case
            "01c180", // second 0x00 case
            "01c101", "01c102", "01c103", "01c104", "01c105", "01c106", "01c107", "01c108",
            "01c109", "01c10a", "01c10b", "01c110",
        ];

        for reason in all_reasons {
            let reason = hex::decode(reason).unwrap();
            let message = P2PMessage::decode(&mut &reason[..]).unwrap();
            let P2PMessage::Disconnect(_) = message else {
                panic!("expected a disconnect message");
            };
        }
    }

    #[test]
    fn test_decode_disconnect_requested() {
        let reason = "0100";
        let reason = hex::decode(reason).unwrap();
        match P2PMessage::decode(&mut &reason[..]).unwrap() {
            P2PMessage::Disconnect(DisconnectReason::DisconnectRequested) => {}
            _ => {
                unreachable!()
            }
        }
    }
}
