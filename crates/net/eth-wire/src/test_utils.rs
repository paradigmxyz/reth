//! Utilities for testing p2p protocol.

use crate::{
    EthVersion, HelloMessageWithProtocols, P2PStream, ProtocolVersion, Status, UnauthedP2PStream,
};
use alloy_chains::Chain;
use reth_discv4::DEFAULT_DISCOVERY_PORT;
use reth_ecies::util::pk2id;
use reth_primitives::{ForkFilter, Head, B256, U256};
use secp256k1::{SecretKey, SECP256K1};
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio_util::codec::{Decoder, Framed, LengthDelimitedCodec};

pub type P2pPassthroughTcpStream = P2PStream<Framed<TcpStream, LengthDelimitedCodec>>;

/// Returns a new testing `HelloMessage` and new secretkey
pub fn eth_hello() -> (HelloMessageWithProtocols, SecretKey) {
    let server_key = SecretKey::new(&mut rand::thread_rng());
    let protocols = vec![EthVersion::Eth67.into()];
    let hello = HelloMessageWithProtocols {
        protocol_version: ProtocolVersion::V5,
        client_version: "eth/1.0.0".to_string(),
        protocols,
        port: DEFAULT_DISCOVERY_PORT,
        id: pk2id(&server_key.public_key(SECP256K1)),
    };
    (hello, server_key)
}

/// Returns testing eth handshake status and fork filter.
pub fn eth_handshake() -> (Status, ForkFilter) {
    let genesis = B256::random();
    let fork_filter = ForkFilter::new(Head::default(), genesis, 0, Vec::new());

    let status = Status {
        version: EthVersion::Eth67 as u8,
        chain: Chain::mainnet(),
        total_difficulty: U256::ZERO,
        blockhash: B256::random(),
        genesis,
        // Pass the current fork id.
        forkid: fork_filter.current(),
    };
    (status, fork_filter)
}

/// Connects to a remote node and returns an authenticated `P2PStream` with the remote node.
pub async fn connect_passthrough(
    addr: SocketAddr,
    client_hello: HelloMessageWithProtocols,
) -> P2pPassthroughTcpStream {
    let outgoing = TcpStream::connect(addr).await.unwrap();
    let sink = crate::PassthroughCodec::default().framed(outgoing);
    let (p2p_stream, _) = UnauthedP2PStream::new(sink).handshake(client_hello).await.unwrap();

    p2p_stream
}

/// A Rplx subprotocol for testing
pub mod proto {
    use super::*;
    use crate::{capability::Capability, protocol::Protocol};
    use bytes::{Buf, BufMut, BytesMut};

    /// Returns a new testing `HelloMessage` with eth and the test protocol
    pub fn test_hello() -> (HelloMessageWithProtocols, SecretKey) {
        let mut handshake = eth_hello();
        handshake.0.protocols.push(TestProtoMessage::protocol());
        handshake
    }

    #[repr(u8)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum TestProtoMessageId {
        Ping = 0x00,
        Pong = 0x01,
        Message = 0x02,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum TestProtoMessageKind {
        Message(String),
        Ping,
        Pong,
    }

    /// An `test` protocol message, containing a message ID and payload.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct TestProtoMessage {
        pub message_type: TestProtoMessageId,
        pub message: TestProtoMessageKind,
    }

    impl TestProtoMessage {
        /// Returns the capability for the `test` protocol.
        pub fn capability() -> Capability {
            Capability::new_static("test", 1)
        }

        /// Returns the protocol for the `test` protocol.
        pub fn protocol() -> Protocol {
            Protocol::new(Self::capability(), 3)
        }

        /// Creates a ping message
        pub fn ping() -> Self {
            Self { message_type: TestProtoMessageId::Ping, message: TestProtoMessageKind::Ping }
        }

        /// Creates a pong message
        pub fn pong() -> Self {
            Self { message_type: TestProtoMessageId::Pong, message: TestProtoMessageKind::Pong }
        }

        /// Creates a message
        pub fn message(msg: impl Into<String>) -> Self {
            Self {
                message_type: TestProtoMessageId::Message,
                message: TestProtoMessageKind::Message(msg.into()),
            }
        }

        /// Creates a new `TestProtoMessage` with the given message ID and payload.
        pub fn encoded(&self) -> BytesMut {
            let mut buf = BytesMut::new();
            buf.put_u8(self.message_type as u8);
            match &self.message {
                TestProtoMessageKind::Ping => {}
                TestProtoMessageKind::Pong => {}
                TestProtoMessageKind::Message(msg) => {
                    buf.put(msg.as_bytes());
                }
            }
            buf
        }

        /// Decodes a `TestProtoMessage` from the given message buffer.
        pub fn decode_message(buf: &mut &[u8]) -> Option<Self> {
            if buf.is_empty() {
                return None
            }
            let id = buf[0];
            buf.advance(1);
            let message_type = match id {
                0x00 => TestProtoMessageId::Ping,
                0x01 => TestProtoMessageId::Pong,
                0x02 => TestProtoMessageId::Message,
                _ => return None,
            };
            let message = match message_type {
                TestProtoMessageId::Ping => TestProtoMessageKind::Ping,
                TestProtoMessageId::Pong => TestProtoMessageKind::Pong,
                TestProtoMessageId::Message => {
                    TestProtoMessageKind::Message(String::from_utf8_lossy(&buf[..]).into_owned())
                }
            };
            Some(Self { message_type, message })
        }
    }
}
