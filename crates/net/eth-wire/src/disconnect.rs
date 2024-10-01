//! Disconnect

use std::future::Future;

use futures::{Sink, SinkExt};
use reth_ecies::stream::ECIESStream;
use reth_eth_wire_types::DisconnectReason;
use tokio::io::AsyncWrite;
use tokio_util::codec::{Encoder, Framed};

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
    use alloy_primitives::hex;
    use alloy_rlp::{Decodable, Encodable};

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
