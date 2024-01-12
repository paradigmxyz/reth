#![allow(unreachable_pub)]
//! Testing gossiping of transactions.

use crate::multiplex::proto::{PingPongProtoMessage, PingPongProtoMessageKind};
use futures::{Stream, StreamExt};
use reth_eth_wire::{
    capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol,
};
use reth_network::{
    protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler},
    test_utils::Testnet,
};
use reth_network_api::Direction;
use reth_primitives::BytesMut;
use reth_provider::test_utils::MockEthProvider;
use reth_rpc_types::PeerId;
use std::{
    net::SocketAddr,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A simple Rplx subprotocol for
mod proto {
    use super::*;
    use reth_eth_wire::capability::Capability;
    use reth_primitives::{Buf, BufMut};

    #[repr(u8)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PingPongProtoMessageId {
        Ping = 0x00,
        Pong = 0x01,
        PingMessage = 0x02,
        PongMessage = 0x03,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum PingPongProtoMessageKind {
        Ping,
        Pong,
        PingMessage(String),
        PongMessage(String),
    }

    /// An protocol message, containing a message ID and payload.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct PingPongProtoMessage {
        pub message_type: PingPongProtoMessageId,
        pub message: PingPongProtoMessageKind,
    }

    impl PingPongProtoMessage {
        /// Returns the capability for the `ping` protocol.
        pub fn capability() -> Capability {
            Capability::new_static("ping", 1)
        }

        /// Returns the protocol for the `test` protocol.
        pub fn protocol() -> Protocol {
            Protocol::new(Self::capability(), 4)
        }

        /// Creates a ping message
        pub fn ping() -> Self {
            Self {
                message_type: PingPongProtoMessageId::Ping,
                message: PingPongProtoMessageKind::Ping,
            }
        }

        /// Creates a pong message
        pub fn pong() -> Self {
            Self {
                message_type: PingPongProtoMessageId::Pong,
                message: PingPongProtoMessageKind::Pong,
            }
        }

        /// Creates a ping message
        pub fn ping_message(msg: impl Into<String>) -> Self {
            Self {
                message_type: PingPongProtoMessageId::PingMessage,
                message: PingPongProtoMessageKind::PingMessage(msg.into()),
            }
        }
        /// Creates a ping message
        pub fn pong_message(msg: impl Into<String>) -> Self {
            Self {
                message_type: PingPongProtoMessageId::PongMessage,
                message: PingPongProtoMessageKind::PongMessage(msg.into()),
            }
        }

        /// Creates a new `TestProtoMessage` with the given message ID and payload.
        pub fn encoded(&self) -> BytesMut {
            let mut buf = BytesMut::new();
            buf.put_u8(self.message_type as u8);
            match &self.message {
                PingPongProtoMessageKind::Ping => {}
                PingPongProtoMessageKind::Pong => {}
                PingPongProtoMessageKind::PingMessage(msg) => {
                    buf.put(msg.as_bytes());
                }
                PingPongProtoMessageKind::PongMessage(msg) => {
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
                0x00 => PingPongProtoMessageId::Ping,
                0x01 => PingPongProtoMessageId::Pong,
                0x02 => PingPongProtoMessageId::PingMessage,
                0x03 => PingPongProtoMessageId::PongMessage,
                _ => return None,
            };
            let message = match message_type {
                PingPongProtoMessageId::Ping => PingPongProtoMessageKind::Ping,
                PingPongProtoMessageId::Pong => PingPongProtoMessageKind::Pong,
                PingPongProtoMessageId::PingMessage => PingPongProtoMessageKind::PingMessage(
                    String::from_utf8_lossy(&buf[..]).into_owned(),
                ),
                PingPongProtoMessageId::PongMessage => PingPongProtoMessageKind::PongMessage(
                    String::from_utf8_lossy(&buf[..]).into_owned(),
                ),
            };
            Some(Self { message_type, message })
        }
    }
}

#[derive(Debug)]
struct PingPongProtoHandler {
    state: ProtocolState,
}

impl ProtocolHandler for PingPongProtoHandler {
    type ConnectionHandler = PingPongConnectionHandler;

    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        Some(PingPongConnectionHandler { state: self.state.clone() })
    }

    fn on_outgoing(
        &self,
        _socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        Some(PingPongConnectionHandler { state: self.state.clone() })
    }
}

#[derive(Clone, Debug)]
struct ProtocolState {
    events: mpsc::UnboundedSender<ProtocolEvent>,
}

#[derive(Debug)]
enum ProtocolEvent {
    Established {
        #[allow(dead_code)]
        direction: Direction,
        peer_id: PeerId,
        to_connection: mpsc::UnboundedSender<Command>,
    },
}

enum Command {
    /// Send a ping message to the peer.
    PingMessage {
        msg: String,
        /// The response will be sent to this channel.
        response: oneshot::Sender<String>,
    },
}

struct PingPongConnectionHandler {
    state: ProtocolState,
}

impl ConnectionHandler for PingPongConnectionHandler {
    type Connection = PingPongProtoConnection;

    fn protocol(&self) -> Protocol {
        PingPongProtoMessage::protocol()
    }

    fn on_unsupported_by_peer(
        self,
        _supported: &SharedCapabilities,
        _direction: Direction,
        _peer_id: PeerId,
    ) -> OnNotSupported {
        OnNotSupported::KeepAlive
    }

    fn into_connection(
        self,
        direction: Direction,
        _peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Self::Connection {
        let (tx, rx) = mpsc::unbounded_channel();
        self.state
            .events
            .send(ProtocolEvent::Established { direction, peer_id: _peer_id, to_connection: tx })
            .ok();
        PingPongProtoConnection {
            conn,
            initial_ping: direction.is_outgoing().then(PingPongProtoMessage::ping),
            commands: UnboundedReceiverStream::new(rx),
            pending_pong: None,
        }
    }
}

struct PingPongProtoConnection {
    conn: ProtocolConnection,
    initial_ping: Option<PingPongProtoMessage>,
    commands: UnboundedReceiverStream<Command>,
    pending_pong: Option<oneshot::Sender<String>>,
}

impl Stream for PingPongProtoConnection {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(initial_ping) = this.initial_ping.take() {
            return Poll::Ready(Some(initial_ping.encoded()))
        }

        loop {
            if let Poll::Ready(Some(cmd)) = this.commands.poll_next_unpin(cx) {
                return match cmd {
                    Command::PingMessage { msg, response } => {
                        this.pending_pong = Some(response);
                        Poll::Ready(Some(PingPongProtoMessage::ping_message(msg).encoded()))
                    }
                }
            }
            let Some(msg) = ready!(this.conn.poll_next_unpin(cx)) else { return Poll::Ready(None) };

            let Some(msg) = PingPongProtoMessage::decode_message(&mut &msg[..]) else {
                return Poll::Ready(None)
            };

            match msg.message {
                PingPongProtoMessageKind::Ping => {
                    return Poll::Ready(Some(PingPongProtoMessage::pong().encoded()))
                }
                PingPongProtoMessageKind::Pong => {}
                PingPongProtoMessageKind::PingMessage(msg) => {
                    return Poll::Ready(Some(PingPongProtoMessage::pong_message(msg).encoded()))
                }
                PingPongProtoMessageKind::PongMessage(msg) => {
                    if let Some(sender) = this.pending_pong.take() {
                        sender.send(msg).ok();
                    }
                    continue
                }
            }

            return Poll::Pending
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_proto_multiplex() {
    reth_tracing::init_test_tracing();
    let provider = MockEthProvider::default();
    let mut net = Testnet::create_with(2, provider.clone()).await;

    let (tx, mut from_peer0) = mpsc::unbounded_channel();
    net.peers_mut()[0]
        .add_rlpx_sub_protocol(PingPongProtoHandler { state: ProtocolState { events: tx } });

    let (tx, mut from_peer1) = mpsc::unbounded_channel();
    net.peers_mut()[1]
        .add_rlpx_sub_protocol(PingPongProtoHandler { state: ProtocolState { events: tx } });

    let handle = net.spawn();
    // connect all the peers
    handle.connect_peers().await;

    let peer0_to_peer1 = from_peer0.recv().await.unwrap();
    let peer0_conn = match peer0_to_peer1 {
        ProtocolEvent::Established { direction: _, peer_id, to_connection } => {
            assert_eq!(peer_id, *handle.peers()[1].peer_id());
            to_connection
        }
    };

    let peer1_to_peer0 = from_peer1.recv().await.unwrap();
    let peer1_conn = match peer1_to_peer0 {
        ProtocolEvent::Established { direction: _, peer_id, to_connection } => {
            assert_eq!(peer_id, *handle.peers()[0].peer_id());
            to_connection
        }
    };

    let (tx, rx) = oneshot::channel();
    // send a ping message from peer0 to peer1
    peer0_conn.send(Command::PingMessage { msg: "hello!".to_string(), response: tx }).unwrap();

    let response = rx.await.unwrap();
    assert_eq!(response, "hello!");

    let (tx, rx) = oneshot::channel();
    // send a ping message from peer1 to peer0
    peer1_conn
        .send(Command::PingMessage { msg: "hello from peer1!".to_string(), response: tx })
        .unwrap();

    let response = rx.await.unwrap();
    assert_eq!(response, "hello from peer1!");
}
