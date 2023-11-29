//! Utilities for testing p2p protocol.

use crate::{
    EthVersion, HelloMessageWithProtocols, P2PStream, ProtocolVersion, Status, UnauthedP2PStream,
};
use reth_discv4::DEFAULT_DISCOVERY_PORT;
use reth_ecies::util::pk2id;
use reth_primitives::{Chain, ForkFilter, Head, B256, U256};
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
