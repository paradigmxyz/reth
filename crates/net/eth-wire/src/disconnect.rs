//! Disconnect

use alloy_rlp::{Decodable, Encodable, Error as RlpError, Header};
use futures::{Sink, SinkExt};
use reth_codecs::derive_arbitrary;
use reth_ecies::stream::ECIESStream;
use reth_primitives::bytes::{Buf, BufMut};
use std::future::Future;
use thiserror::Error;
use tokio::io::AsyncWrite;
use tokio_util::codec::{Encoder, Framed};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// RLPx disconnect reason.
#[derive_arbitrary(rlp)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, derive_more::Display)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum DisconnectReason {
    /// Disconnect requested by the local node or remote peer.
    #[default]
    #[display(fmt = "disconnect requested")]
    DisconnectRequested = 0x00,
    /// TCP related error
    #[display(fmt = "TCP sub-system error")]
    TcpSubsystemError = 0x01,
    /// Breach of protocol at the transport or p2p level
    #[display(fmt = "breach of protocol, e.g. a malformed message, bad RLP, etc.")]
    ProtocolBreach = 0x02,
    /// Node has no matching protocols.
    #[display(fmt = "useless peer")]
    UselessPeer = 0x03,
    /// Either the remote or local node has too many peers.
    #[display(fmt = "too many peers")]
    TooManyPeers = 0x04,
    /// Already connected to the peer.
    #[display(fmt = "already connected")]
    AlreadyConnected = 0x05,
    /// `p2p` protocol version is incompatible
    #[display(fmt = "incompatible P2P protocol version")]
    IncompatibleP2PProtocolVersion = 0x06,
    /// Received a null node identity.
    #[display(fmt = "null node identity received - this is automatically invalid")]
    NullNodeIdentity = 0x07,
    /// Reason when the client is shutting down.
    #[display(fmt = "client quitting")]
    ClientQuitting = 0x08,
    /// When the received handshake's identify is different from what is expected.
    #[display(fmt = "unexpected identity in handshake")]
    UnexpectedHandshakeIdentity = 0x09,
    /// The node is connected to itself
    #[display(fmt = "identity is the same as this node (i.e. connected to itself)")]
    ConnectedToSelf = 0x0a,
    /// Peer or local node did not respond to a ping in time.
    #[display(fmt = "ping timeout")]
    PingTimeout = 0x0b,
    /// Peer or local node violated a subprotocol-specific rule.
    #[display(fmt = "some other reason specific to a subprotocol")]
    SubprotocolSpecific = 0x10,
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
            return Err(RlpError::InputTooShort)
        } else if buf.len() > 2 {
            return Err(RlpError::Overflow)
        }

        if buf.len() > 1 {
            // this should be a list, so decode the list header. this should advance the buffer so
            // buf[0] is the first (and only) element of the list.
            let header = Header::decode(buf)?;

            if !header.list {
                return Err(RlpError::UnexpectedString)
            }

            if header.payload_length != 1 {
                return Err(RlpError::ListLengthMismatch { expected: 1, got: header.payload_length })
            }
        }

        // geth rlp encodes [`DisconnectReason::DisconnectRequested`] as 0x00 and not as empty
        // string 0x80
        if buf[0] == 0x00 {
            buf.advance(1);
            Ok(Self::DisconnectRequested)
        } else {
            Self::try_from(u8::decode(buf)?)
                .map_err(|_| RlpError::Custom("unknown disconnect reason"))
        }
    }
}

/// This trait is meant to allow higher level protocols like `eth` to disconnect from a peer, using
/// lower-level disconnect functions (such as those that exist in the `p2p` protocol) if the
/// underlying stream supports it.
pub trait CanDisconnect<T>: Sink<T> + Unpin {
    /// Disconnects from the underlying stream, using a [`DisconnectReason`] as disconnect
    /// information if the stream implements a protocol that can carry the additional disconnect
    /// metadata.
    fn disconnect(
        &mut self,
        reason: DisconnectReason,
    ) -> impl Future<Output = Result<(), <Self as Sink<T>>::Error>> + Send;
}

// basic impls for things like Framed<TcpStream, etc>
impl<T, I, U> CanDisconnect<I> for Framed<T, U>
where
    T: AsyncWrite + Unpin + Send,
    U: Encoder<I> + Send,
{
    async fn disconnect(
        &mut self,
        _reason: DisconnectReason,
    ) -> Result<(), <Self as Sink<I>>::Error> {
        self.close().await
    }
}

impl<S> CanDisconnect<bytes::Bytes> for ECIESStream<S>
where
    S: AsyncWrite + Unpin + Send,
{
    async fn disconnect(&mut self, _reason: DisconnectReason) -> Result<(), std::io::Error> {
        self.close().await
    }
}

#[cfg(test)]
mod tests {
    use crate::{p2pstream::P2PMessage, DisconnectReason};
    use alloy_rlp::{Decodable, Encodable};
    use reth_primitives::hex;

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
    fn test_reason_zero_length_list() {
        let list_with_zero_length = hex::decode("c000").unwrap();
        let res = DisconnectReason::decode(&mut &list_with_zero_length[..]);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "unexpected list length (got 0, expected 1)")
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
