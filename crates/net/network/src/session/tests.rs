//! Tests for the session handshake.

use super::*;
use crate::{
    error::SessionError,
    protocol::{ConnectionHandler, DynConnectionHandler},
};
use alloy_primitives::bytes::BytesMut;
use futures::{stream::Pending, StreamExt};
use reth_chainspec::MAINNET;
use reth_eth_wire::{
    capability::SharedCapabilities, handshake::EthHandshake, multiplex::ProtocolConnection,
    protocol::Protocol, Capability, EthNetworkPrimitives, StatusBuilder, UnauthedEthStream,
};
use reth_eth_wire_types::message::MAX_MESSAGE_SIZE;
use reth_ethereum_forks::EthereumHardfork;
use reth_network_peers::pk2id;
use secp256k1::SECP256K1;
use std::sync::Mutex;
use tokio::net::TcpListener;

/// Extra `RLPx` protocol that records which identity the session hands to it.
#[derive(Clone, Debug, Default)]
struct ProtocolSpy {
    seen: Arc<Mutex<SeenPeers>>,
}

/// The peers a [`ProtocolSpy`] was told about, per callback.
#[derive(Debug, Default)]
struct SeenPeers {
    connected: Vec<PeerId>,
    unsupported: Vec<PeerId>,
}

impl ProtocolSpy {
    /// The peers passed to [`ConnectionHandler::into_connection`].
    fn connected(&self) -> Vec<PeerId> {
        self.seen.lock().unwrap().connected.clone()
    }

    /// Whether the session named a peer to this protocol through either callback.
    fn saw_any_peer(&self) -> bool {
        let seen = self.seen.lock().unwrap();
        !seen.connected.is_empty() || !seen.unsupported.is_empty()
    }
}

/// The name sorts after `eth` so that `eth` keeps the first message id offset on both ends.
fn spy_protocol() -> Protocol {
    Protocol::new(Capability::new_static("tst", 1), 1)
}

impl ConnectionHandler for ProtocolSpy {
    type Connection = Pending<BytesMut>;

    fn protocol(&self) -> Protocol {
        spy_protocol()
    }

    fn on_unsupported_by_peer(
        self,
        _supported: &SharedCapabilities,
        _direction: Direction,
        peer_id: PeerId,
    ) -> OnNotSupported {
        self.seen.lock().unwrap().unsupported.push(peer_id);
        OnNotSupported::KeepAlive
    }

    fn into_connection(
        self,
        _direction: Direction,
        peer_id: PeerId,
        _conn: ProtocolConnection,
    ) -> Self::Connection {
        self.seen.lock().unwrap().connected.push(peer_id);
        futures::stream::pending()
    }
}

fn new_peer() -> (SecretKey, PeerId) {
    let (key, pk) = SECP256K1.generate_keypair(&mut rand_08::thread_rng());
    (key, pk2id(&pk))
}

fn status() -> UnifiedStatus {
    StatusBuilder::default().build()
}

fn fork_filter() -> ForkFilter {
    MAINNET
        .hardfork_fork_filter(EthereumHardfork::Frontier)
        .expect("The Frontier fork filter should exist on mainnet")
}

/// The hello a remote sends us, announcing `id` and the spy protocol.
fn remote_hello(id: PeerId) -> HelloMessageWithProtocols {
    let mut hello = HelloMessageWithProtocols::builder(id).build();
    hello.try_add_protocol(spy_protocol()).unwrap();
    hello
}

fn spy_handlers(spy: &ProtocolSpy) -> RlpxSubProtocolHandlers {
    RlpxSubProtocolHandlers(vec![Box::new(spy.clone()) as Box<dyn DynConnectionHandler>])
}

/// Asserts the event reports an identity mismatch of `got` against `expected` and that the
/// peers manager will treat it as fatal.
fn assert_identity_mismatch(
    event: PendingSessionEvent<EthNetworkPrimitives>,
    got: PeerId,
    expected: PeerId,
) {
    let PendingSessionEvent::Disconnected { error: Some(err), .. } = event else {
        panic!("unexpected event {event:?}")
    };
    let PendingSessionHandshakeError::UnexpectedHandshakeIdentity(ref mismatch) = err else {
        panic!("unexpected error {err:?}")
    };
    assert_eq!(mismatch.got, got);
    assert_eq!(mismatch.expected, expected);
    assert_eq!(err.as_disconnected(), Some(DisconnectReason::UnexpectedHandshakeIdentity));
    assert!(err.is_fatal_protocol_error());
}

/// An ECIES-authenticated peer that announces a different node id in its `Hello` must not be
/// able to make the session, or any extra protocol installed on it, act on that identity.
#[tokio::test(flavor = "multi_thread")]
async fn incoming_hello_with_spoofed_identity_is_rejected() {
    let (local_key, local_id) = new_peer();
    let (attacker_key, attacker_id) = new_peer();
    let (_, victim_id) = new_peer();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let spy = ProtocolSpy::default();
    let handlers = spy_handlers(&spy);

    let (events_tx, mut events_rx) = mpsc::channel(1);
    let (_disconnect_tx, disconnect_rx) = oneshot::channel();

    tokio::spawn(async move {
        let (incoming, remote_addr) = listener.accept().await.unwrap();
        start_pending_incoming_session::<EthNetworkPrimitives>(
            Arc::new(EthHandshake::default()),
            MAX_MESSAGE_SIZE,
            disconnect_rx,
            SessionId(0),
            incoming,
            events_tx,
            remote_addr,
            local_key,
            HelloMessageWithProtocols::builder(local_id).build(),
            status(),
            fork_filter(),
            handlers,
        )
        .await
    });

    let attacker = tokio::spawn(async move {
        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let ecies = ECIESStream::connect(outgoing, attacker_key, local_id).await.unwrap();
        let (mut p2p_stream, _) =
            UnauthedP2PStream::new(ecies).handshake(remote_hello(victim_id)).await.unwrap();
        p2p_stream.next().await.unwrap().unwrap_err().as_disconnected()
    });

    assert_identity_mismatch(events_rx.recv().await.unwrap(), victim_id, attacker_id);
    assert!(events_rx.recv().await.is_none(), "no session may be established");
    assert_eq!(attacker.await.unwrap(), Some(DisconnectReason::UnexpectedHandshakeIdentity));

    assert!(!spy.saw_any_peer(), "the spoofed identity must not reach an extra protocol");
}

/// The same binding applies when we dial out: the remote proved possession of the key we
/// dialed, so its `Hello` cannot rename the session to a different peer.
#[tokio::test(flavor = "multi_thread")]
async fn outgoing_hello_with_spoofed_identity_is_rejected() {
    let (local_key, local_id) = new_peer();
    let (remote_key, remote_id) = new_peer();
    let (_, victim_id) = new_peer();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let remote_addr = listener.local_addr().unwrap();
    let spy = ProtocolSpy::default();
    let handlers = spy_handlers(&spy);

    let (events_tx, mut events_rx) = mpsc::channel(1);
    let (_disconnect_tx, disconnect_rx) = oneshot::channel();

    let remote = tokio::spawn(async move {
        let (incoming, _) = listener.accept().await.unwrap();
        let ecies = ECIESStream::incoming(incoming, remote_key).await.unwrap();
        let (mut p2p_stream, _) =
            UnauthedP2PStream::new(ecies).handshake(remote_hello(victim_id)).await.unwrap();
        p2p_stream.next().await.unwrap().unwrap_err().as_disconnected()
    });

    tokio::spawn(async move {
        start_pending_outbound_session::<EthNetworkPrimitives>(
            Arc::new(EthHandshake::default()),
            MAX_MESSAGE_SIZE,
            disconnect_rx,
            events_tx,
            SessionId(0),
            remote_addr,
            remote_id,
            local_key,
            HelloMessageWithProtocols::builder(local_id).build(),
            status(),
            fork_filter(),
            handlers,
        )
        .await
    });

    assert_identity_mismatch(events_rx.recv().await.unwrap(), victim_id, remote_id);
    assert!(events_rx.recv().await.is_none(), "no session may be established");
    assert_eq!(remote.await.unwrap(), Some(DisconnectReason::UnexpectedHandshakeIdentity));

    assert!(!spy.saw_any_peer(), "the spoofed identity must not reach an extra protocol");
}

/// A remote whose `Hello` agrees with its ECIES identity gets a session, and the extra
/// protocol is handed that same identity.
#[tokio::test(flavor = "multi_thread")]
async fn matching_identity_establishes_session() {
    let (local_key, local_id) = new_peer();
    let (remote_key, remote_id) = new_peer();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let spy = ProtocolSpy::default();
    let handlers = spy_handlers(&spy);

    let (events_tx, mut events_rx) = mpsc::channel(1);
    let (_disconnect_tx, disconnect_rx) = oneshot::channel();

    tokio::spawn(async move {
        let (incoming, remote_addr) = listener.accept().await.unwrap();
        start_pending_incoming_session::<EthNetworkPrimitives>(
            Arc::new(EthHandshake::default()),
            MAX_MESSAGE_SIZE,
            disconnect_rx,
            SessionId(0),
            incoming,
            events_tx,
            remote_addr,
            local_key,
            HelloMessageWithProtocols::builder(local_id).build(),
            status(),
            fork_filter(),
            handlers,
        )
        .await
    });

    let (keep_alive_tx, keep_alive_rx) = oneshot::channel();
    let remote = tokio::spawn(async move {
        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let ecies = ECIESStream::connect(outgoing, remote_key, local_id).await.unwrap();
        let (p2p_stream, _) =
            UnauthedP2PStream::new(ecies).handshake(remote_hello(remote_id)).await.unwrap();

        let mut status = status();
        status.set_eth_version(p2p_stream.shared_capabilities().eth_version().unwrap());
        let _eth_stream = UnauthedEthStream::new(p2p_stream)
            .handshake::<EthNetworkPrimitives>(status, fork_filter())
            .await
            .unwrap();

        let _ = keep_alive_rx.await;
    });

    let (peer_id, mut conn) = match events_rx.recv().await.unwrap() {
        PendingSessionEvent::Established { peer_id, conn, .. } => (peer_id, conn),
        event => panic!("unexpected event {event:?}"),
    };
    assert_eq!(peer_id, remote_id);
    assert_eq!(spy.connected(), vec![remote_id]);

    // From here on a real session owns and polls the connection. Without that the status we queued
    // for the remote never reaches the socket and its handshake times out.
    let session = tokio::spawn(async move { conn.next().await });

    let _ = keep_alive_tx.send(());
    remote.await.unwrap();
    session.abort();
}
