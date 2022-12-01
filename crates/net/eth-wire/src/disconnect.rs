use std::fmt::Display;

use bytes::Buf;
use reth_rlp::{Decodable, DecodeError, Encodable};

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

        write!(f, "{message}")
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
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        // disconnect reasons are snappy encoded as follows:
        // [0x02, 0x04, 0xc1, reason as u8]
        // this is 3 bytes
        out.put_u8(0x02);
        out.put_u8(0x04);
        out.put_u8(0xc1);
        out.put_u8(*self as u8);
    }
    fn length(&self) -> usize {
        // disconnect reasons are snappy encoded as follows:
        // [0x02, 0x04, 0xc1, reason as u8]
        // this is 4 bytes
        4
    }
}

impl Decodable for DisconnectReason {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        if buf.len() < 4 {
            return Err(DecodeError::Custom("disconnect reason should have 4 bytes"))
        }
        println!("{:x?}", buf);

        let first = buf[0];
        if first != 0x02 {
            return Err(DecodeError::Custom("invalid disconnect reason - invalid snappy header"))
        }

        let second = buf[1];
        if second != 0x04 {
            // TODO: make sure this error message is correct
            return Err(DecodeError::Custom("invalid disconnect reason - invalid snappy header"))
        }

        let third = buf[2];
        if third != 0xc1 {
            return Err(DecodeError::Custom("invalid disconnect reason - invalid rlp header"))
        }

        let reason = buf[3];
        let reason = DisconnectReason::try_from(reason)
            .map_err(|_| DecodeError::Custom("unknown disconnect reason"))?;
        buf.advance(4);
        Ok(reason)
    }
}

#[cfg(test)]
mod tests {
    use crate::{p2pstream::P2PMessage, DisconnectReason};
    use reth_rlp::{Decodable, Encodable};

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
    fn test_reason_too_short() {
        assert!(DisconnectReason::decode(&mut &[0u8][..]).is_err())
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
