//! Tests for the session handshake.

use super::*;
use crate::{
    error::SessionError,
    protocol::{ConnectionHandler, DynConnectionHandler},
};
use alloy_primitives::bytes::{Bytes, BytesMut};
use futures::{stream::BoxStream, SinkExt, StreamExt};
use reth_chainspec::MAINNET;
use reth_eth_wire::{
    capability::SharedCapabilities,
    errors::P2PStreamError,
    handshake::EthHandshake,
    multiplex::ProtocolConnection,
    protocol::{Protocol, ProtocolIngressLimits},
    Capability, EthNetworkPrimitives, StatusBuilder, UnauthedEthStream,
};
use reth_eth_wire_types::message::MAX_MESSAGE_SIZE;
use reth_ethereum_forks::EthereumHardfork;
use reth_network_peers::pk2id;
use secp256k1::SECP256K1;
use std::{sync::Mutex, task::Waker};
use tokio::{net::TcpListener, sync::Notify};

/// Extra `RLPx` protocol that records which identity the session hands to it.
#[derive(Clone, Debug, Default)]
struct ProtocolSpy {
    seen: Arc<Mutex<SeenPeers>>,
    limits: ProtocolIngressLimits,
    message_received: Arc<Notify>,
}

/// The peers a [`ProtocolSpy`] was told about, per callback.
#[derive(Debug, Default)]
struct SeenPeers {
    connected: Vec<PeerId>,
    unsupported: Vec<PeerId>,
    messages: Vec<BytesMut>,
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
    type Connection = BoxStream<'static, BytesMut>;

    fn protocol(&self) -> Protocol {
        spy_protocol()
    }

    fn inbound_limits(&self) -> ProtocolIngressLimits {
        self.limits
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
        conn: ProtocolConnection,
    ) -> Self::Connection {
        self.seen.lock().unwrap().connected.push(peer_id);
        conn.filter_map(move |message| {
            self.seen.lock().unwrap().messages.push(message);
            self.message_received.notify_one();
            futures::future::ready(None)
        })
        .boxed()
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
        let (mut p2p_stream, _) =
            UnauthedP2PStream::new(ecies).handshake(remote_hello(remote_id)).await.unwrap();

        let offset = p2p_stream
            .shared_capabilities()
            .find(&spy_protocol().cap)
            .unwrap()
            .relative_message_id_offset();
        p2p_stream.send(Bytes::from(vec![offset, 0x80])).await.unwrap();

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

    // From here on a real session owns and polls the connection. Without that the status we queued
    // for the remote never reaches the socket and its handshake times out.
    let session = tokio::spawn(async move { conn.next().await });

    tokio::time::timeout(Duration::from_secs(5), spy.message_received.notified()).await.unwrap();
    assert_eq!(spy.connected(), vec![remote_id]);
    assert_eq!(spy.seen.lock().unwrap().messages, vec![BytesMut::from(&[0, 0x80][..])]);
    let _ = keep_alive_tx.send(());
    remote.await.unwrap();
    session.abort();
}

/// A remote that completes `Hello` but never sends `Status` must not reach an extra protocol,
/// even if it already sends messages of that protocol.
#[tokio::test(flavor = "multi_thread")]
async fn extra_protocol_waits_for_status_handshake() {
    for (max_frame_bytes, frame_bytes, frame_count) in
        [(2, 2, 1), (1, 2, 1), (102_400, 102_400, 3), (2, 2, 65)]
    {
        let (local_key, local_id) = new_peer();
        let (remote_key, remote_id) = new_peer();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let spy = ProtocolSpy {
            limits: ProtocolIngressLimits::new(max_frame_bytes),
            ..Default::default()
        };
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
            let (mut p2p_stream, _) =
                UnauthedP2PStream::new(ecies).handshake(remote_hello(remote_id)).await.unwrap();

            let offset = p2p_stream
                .shared_capabilities()
                .find(&spy_protocol().cap)
                .unwrap()
                .relative_message_id_offset();
            for _ in 0..frame_count {
                let mut frame = vec![0x80; frame_bytes];
                frame[0] = offset;
                p2p_stream.send(Bytes::from(frame)).await.unwrap();
            }
            if max_frame_bytes == 2 && frame_count == 1 {
                p2p_stream.disconnect(DisconnectReason::ClientQuitting).await.unwrap();
            } else {
                // Keep unread Status data from causing an early TCP reset before the local
                // handshake reports the ingress-limit error.
                let _ = keep_alive_rx.await;
            }
        });

        let event = events_rx.recv().await.unwrap();
        let PendingSessionEvent::Disconnected {
            error: Some(PendingSessionHandshakeError::Eth(error)),
            ..
        } = event
        else {
            panic!("unexpected event {event:?}")
        };
        if max_frame_bytes == 1 {
            assert!(matches!(
                error,
                EthStreamError::P2PStreamError(P2PStreamError::SubprotocolMessageTooBig {
                    message_size: 2,
                    max_size: 1,
                    ..
                })
            ));
        }
        if frame_count > 1 {
            assert!(matches!(
                error,
                EthStreamError::P2PStreamError(P2PStreamError::SubprotocolInboundBufferFull { .. })
            ));
        }
        let _ = keep_alive_tx.send(());
        remote.await.unwrap();

        assert!(
            spy.connected().is_empty(),
            "the extra protocol must wait for the status handshake"
        );
    }
}

fn session_manager() -> SessionManager<EthNetworkPrimitives> {
    let (key, id) = new_peer();
    SessionManager::new(
        key,
        SessionsConfig::default(),
        Runtime::test(),
        status(),
        remote_hello(id),
        fork_filter(),
        RlpxSubProtocols::default(),
        Arc::new(EthHandshake::default()),
        MAX_MESSAGE_SIZE,
        false,
    )
}

#[tokio::test]
async fn canceled_dial_events_do_not_reach_replacement_sessions() {
    let mut sessions = session_manager();
    let (_, peer_id) = new_peer();
    let remote_addr = "127.0.0.1:8008".parse().unwrap();
    let (disconnect_tx, disconnect_rx) = oneshot::channel();
    let old_id = sessions.next_id();
    sessions.pending_sessions.insert(
        old_id,
        PendingSessionHandle {
            disconnect_tx: Some(disconnect_tx),
            direction: Direction::Outgoing(peer_id),
        },
    );
    sessions.counter.inc_pending_outbound();
    sessions.cancel_pending_outbound(peer_id);
    assert_eq!(disconnect_rx.await, Ok(()));
    assert!(sessions.pending_sessions.is_empty());

    let new_id = sessions.next_id();
    let (disconnect_tx, _disconnect_rx) = oneshot::channel();
    sessions.pending_sessions.insert(
        new_id,
        PendingSessionHandle {
            disconnect_tx: Some(disconnect_tx),
            direction: Direction::Outgoing(peer_id),
        },
    );
    sessions.counter.inc_pending_outbound();
    for error in [None, Some(PendingSessionHandshakeError::Timeout)] {
        sessions
            .pending_sessions_tx
            .send(PendingSessionEvent::Disconnected {
                remote_addr,
                session_id: old_id,
                direction: Direction::Outgoing(peer_id),
                error,
            })
            .await
            .unwrap();
        assert!(sessions.poll(&mut Context::from_waker(Waker::noop()), |_| true).is_pending());
        assert!(sessions.pending_sessions.contains_key(&new_id));
    }
    sessions
        .pending_sessions_tx
        .send(PendingSessionEvent::OutgoingConnectionError {
            remote_addr,
            session_id: old_id,
            peer_id,
            error: std::io::ErrorKind::ConnectionRefused.into(),
        })
        .await
        .unwrap();
    assert!(sessions.poll(&mut Context::from_waker(Waker::noop()), |_| true).is_pending());
    sessions
        .pending_sessions_tx
        .send(PendingSessionEvent::Disconnected {
            remote_addr,
            session_id: new_id,
            direction: Direction::Outgoing(peer_id),
            error: None,
        })
        .await
        .unwrap();
    assert!(matches!(sessions.poll(&mut Context::from_waker(Waker::noop()), |_| true),
        Poll::Ready(SessionEvent::OutgoingPendingSessionClosed { peer_id: id, error: None, .. }) if id == peer_id));
    assert!(sessions.pending_sessions.is_empty());
}

#[tokio::test]
async fn cancellation_interrupts_stalled_ecies() {
    let (key, id) = new_peer();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let remote_addr = listener.local_addr().unwrap();
    let stream = TcpStream::connect(remote_addr).await.unwrap();
    let (_remote, _) = listener.accept().await.unwrap();
    let (disconnect_tx, disconnect_rx) = oneshot::channel();
    let (events, mut rx) = mpsc::channel(1);
    let mut auth = Box::pin(authenticate::<EthNetworkPrimitives>(
        Arc::new(EthHandshake::default()),
        MAX_MESSAGE_SIZE,
        disconnect_rx,
        events,
        stream,
        SessionId(0),
        remote_addr,
        key,
        Direction::Incoming,
        remote_hello(id),
        status(),
        fork_filter(),
        RlpxSubProtocolHandlers::default(),
    ));
    assert!(futures::poll!(auth.as_mut()).is_pending());
    disconnect_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), auth).await.unwrap();
    assert!(matches!(
        rx.recv().await.unwrap(),
        PendingSessionEvent::Disconnected { error: None, .. }
    ));
}

async fn pending_connection_pair(
) -> (SessionManager<EthNetworkPrimitives>, SessionManager<EthNetworkPrimitives>) {
    let mut incoming = session_manager();
    let mut outgoing = session_manager();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    outgoing.dial_outbound(listener.local_addr().unwrap(), incoming.hello_message.id);
    let (stream, addr) =
        tokio::time::timeout(Duration::from_secs(5), listener.accept()).await.unwrap().unwrap();
    incoming.on_incoming(stream, addr).unwrap();
    (incoming, outgoing)
}

#[tokio::test]
async fn canceled_completed_dial_sends_disconnect() {
    let (mut incoming, mut outgoing) = pending_connection_pair().await;
    let event = tokio::time::timeout(Duration::from_secs(5), outgoing.pending_session_rx.next())
        .await
        .unwrap()
        .unwrap();
    let PendingSessionEvent::Established { peer_id, .. } = &event else {
        panic!("expected established")
    };
    let peer_id = *peer_id;
    let remote = tokio::time::timeout(Duration::from_secs(5), incoming.pending_session_rx.next())
        .await
        .unwrap()
        .unwrap();
    let PendingSessionEvent::Established { conn, .. } = remote else {
        panic!("expected established")
    };
    let mut conn = conn.into_inner();
    outgoing.cancel_pending_outbound(peer_id);
    let replacement = outgoing.next_id();
    let (tx, _rx) = oneshot::channel();
    outgoing.pending_sessions.insert(
        replacement,
        PendingSessionHandle { disconnect_tx: Some(tx), direction: Direction::Outgoing(peer_id) },
    );
    outgoing.counter.inc_pending_outbound();
    outgoing.pending_sessions_tx.send(event).await.unwrap();
    assert!(outgoing.poll(&mut Context::from_waker(Waker::noop()), |_| true).is_pending());
    assert!(outgoing.pending_sessions.contains_key(&replacement));
    assert!(outgoing.active_sessions.is_empty());
    let error = tokio::time::timeout(Duration::from_secs(5), conn.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, P2PStreamError::Disconnected(DisconnectReason::AlreadyConnected)));
}

#[tokio::test]
async fn inbound_capacity_rejection_preserves_outbound_attempt() {
    for allow in [false, true] {
        let (mut incoming, mut outgoing) = pending_connection_pair().await;
        let peer_id = outgoing.hello_message.id;
        let outbound_id = incoming.next_id();
        let (tx, mut rx) = oneshot::channel();
        incoming.pending_sessions.insert(
            outbound_id,
            PendingSessionHandle {
                disconnect_tx: Some(tx),
                direction: Direction::Outgoing(peer_id),
            },
        );
        incoming.counter.inc_pending_outbound();
        let event = tokio::time::timeout(
            Duration::from_secs(5),
            futures::future::poll_fn(|cx| {
                incoming.poll(cx, |id| {
                    assert_eq!(id, peer_id);
                    allow
                })
            }),
        )
        .await
        .unwrap();
        if allow {
            assert!(matches!(event, SessionEvent::SessionEstablished { .. }));
            assert_eq!(rx.await, Ok(()));
            assert!(!incoming.pending_sessions.contains_key(&outbound_id));
            assert!(incoming.active_sessions.contains_key(&peer_id));
        } else {
            assert!(matches!(
                event,
                SessionEvent::IncomingPendingSessionClosed { error: None, .. }
            ));
            assert!(matches!(rx.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
            assert!(incoming.pending_sessions.contains_key(&outbound_id));
            assert!(incoming.active_sessions.is_empty());
            let remote =
                tokio::time::timeout(Duration::from_secs(5), outgoing.pending_session_rx.next())
                    .await
                    .unwrap()
                    .unwrap();
            let PendingSessionEvent::Established { conn, .. } = remote else {
                panic!("expected established")
            };
            let mut conn = conn.into_inner();
            let error = tokio::time::timeout(Duration::from_secs(5), conn.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert!(matches!(error, P2PStreamError::Disconnected(DisconnectReason::TooManyPeers)));
        }
    }
}
