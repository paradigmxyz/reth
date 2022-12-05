//! Disconnect

use bytes::Buf;
use reth_rlp::{Decodable, DecodeError, Encodable, EMPTY_LIST_CODE};
use std::fmt::Display;
use thiserror::Error;

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
/// disconnect reason as RLP, and prepends a snappy header to the RLP bytes.
impl Encodable for DisconnectReason {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        // disconnect reasons are snappy encoded as follows:
        // [0x02, 0x04, 0xc1, rlp(reason as u8)]
        // this is 4 bytes
        out.put_u8(0x02);
        out.put_u8(0x04);
        vec![*self as u8].encode(out);
    }
    fn length(&self) -> usize {
        // disconnect reasons are snappy encoded as follows:
        // [0x02, 0x04, 0xc1, rlp(reason as u8)]
        // this is 4 bytes
        4
    }
}

/// The [`Decodable`](reth_rlp::Decodable) implementation for [`DisconnectReason`] assumes that the
/// input is snappy compressed.
impl Decodable for DisconnectReason {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        if buf.len() < 4 {
            return Err(DecodeError::Custom("disconnect reason should have 4 bytes"))
        }

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
        if third != EMPTY_LIST_CODE + 1 {
            return Err(DecodeError::Custom("invalid disconnect reason - invalid rlp header"))
        }

        let reason = u8::decode(&mut &buf[3..])?;
        let reason = DisconnectReason::try_from(reason)
            .map_err(|_| DecodeError::Custom("unknown disconnect reason"))?;
        buf.advance(4);
        Ok(reason)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        p2pstream::{P2PMessage, P2PMessageID},
        DisconnectReason,
    };
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
        assert!(DisconnectReason::decode(&mut &[0u8][..]).is_err())
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
    fn disconnect_snappy_encoding_parity() {
        // encode disconnect using our `Encodable` implementation
        let disconnect = P2PMessage::Disconnect(DisconnectReason::DisconnectRequested);
        let mut disconnect_encoded = Vec::new();
        disconnect.encode(&mut disconnect_encoded);

        let mut disconnect_raw = Vec::new();
        // encode [DisconnectRequested]
        // DisconnectRequested will be converted to 0x80 (encoding of 0) in Encodable::encode
        Encodable::encode(&vec![0x00u8], &mut disconnect_raw);

        let mut snappy_encoder = snap::raw::Encoder::new();
        let disconnect_compressed = snappy_encoder.compress_vec(&disconnect_raw).unwrap();
        let mut disconnect_expected = vec![P2PMessageID::Disconnect as u8];
        disconnect_expected.extend(&disconnect_compressed);

        // ensure that the two encodings are equal
        assert_eq!(
            disconnect_expected, disconnect_encoded,
            "left: {disconnect_expected:#x?}, right: {disconnect_encoded:#x?}"
        );

        // also ensure that the length is correct
        assert_eq!(
            disconnect_expected.len(),
            P2PMessage::Disconnect(DisconnectReason::DisconnectRequested).length()
        );
    }

    #[test]
    fn disconnect_snappy_decoding_parity() {
        // encode disconnect using our `Encodable` implementation
        let disconnect = P2PMessage::Disconnect(DisconnectReason::DisconnectRequested);
        let mut disconnect_encoded = Vec::new();
        disconnect.encode(&mut disconnect_encoded);

        // try to decode using Decodable
        let p2p_message = P2PMessage::decode(&mut &disconnect_encoded[..]).unwrap();
        assert_eq!(p2p_message, P2PMessage::Disconnect(DisconnectReason::DisconnectRequested));

        // finally decode the encoded message with snappy
        let mut snappy_decoder = snap::raw::Decoder::new();

        // the message id is not compressed, only compress the latest bits
        let decompressed = snappy_decoder.decompress_vec(&disconnect_encoded[1..]).unwrap();

        let mut disconnect_raw = Vec::new();
        // encode [DisconnectRequested]
        // DisconnectRequested will be converted to 0x80 (encoding of 0) in Encodable::encode
        Encodable::encode(&vec![0x00u8], &mut disconnect_raw);

        assert_eq!(decompressed, disconnect_raw);
    }
}
