use crate::{capability::Capability, ProtocolVersion};
use reth_primitives::PeerId;
use reth_rlp::{RlpDecodable, RlpEncodable};
use serde::{Deserialize, Serialize};

// TODO: determine if we should allow for the extra fields at the end like EIP-706 suggests
/// Message used in the `p2p` handshake, containing information about the supported RLPx protocol
/// version and capabilities.
#[derive(
    Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Serialize, Deserialize, Default,
)]
pub struct HelloMessage {
    /// The version of the `p2p` protocol.
    pub protocol_version: ProtocolVersion,
    /// Specifies the client software identity, as a human-readable string (e.g.
    /// "Ethereum(++)/1.0.0").
    pub client_version: String,
    /// The list of supported capabilities and their versions.
    pub capabilities: Vec<Capability>,
    /// The port that the client is listening on, zero indicates the client is not listening.
    pub port: u16,
    /// The secp256k1 public key corresponding to the node's private key.
    pub id: PeerId,
}

#[cfg(test)]
mod tests {
    use reth_ecies::util::pk2id;
    use reth_rlp::{Decodable, Encodable, EMPTY_STRING_CODE};
    use secp256k1::{SecretKey, SECP256K1};

    use crate::{
        capability::Capability,
        p2pstream::{P2PMessage, P2PMessageID},
        EthVersion, HelloMessage, ProtocolVersion,
    };

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
        assert_eq!(
            pong_expected, pong_encoded,
            "left: {pong_expected:#x?}, right: {pong_encoded:#x?}"
        );

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
            capabilities: vec![Capability::new("eth".into(), EthVersion::Eth67 as usize)],
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
            capabilities: vec![Capability::new("eth".into(), EthVersion::Eth67 as usize)],
            port: 30303,
            id,
        });

        let mut hello_encoded = Vec::new();
        hello.encode(&mut hello_encoded);

        assert_eq!(hello_encoded.len(), hello.length());
    }

    #[test]
    fn hello_message_id_prefix() {
        // ensure that the hello message id is prefixed
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let id = pk2id(&secret_key.public_key(SECP256K1));
        let hello = P2PMessage::Hello(HelloMessage {
            protocol_version: ProtocolVersion::V5,
            client_version: "reth/0.1.0".to_string(),
            capabilities: vec![Capability::new("eth".into(), EthVersion::Eth67 as usize)],
            port: 30303,
            id,
        });

        let mut hello_encoded = Vec::new();
        hello.encode(&mut hello_encoded);

        // zero is encoded as 0x80, the empty string code in RLP
        assert_eq!(hello_encoded[0], EMPTY_STRING_CODE);
    }
}
