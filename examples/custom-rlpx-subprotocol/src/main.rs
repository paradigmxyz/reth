// This example showcase rlpx subprotocols

// Look closely into this crates/imports(use) that the code is importing

use reth::{builder::NodeHandle, network};

use crate::proto::{CustomRlpxProtoMessage, CustomRlpxProtoMessageKind};
use futures::{Stream, StreamExt};
use reth_eth_wire::{
    capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol,
};
use reth_network::{
    protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler},
    NetworkProtocols,
    test_utils::Testnet,
};
use reth_node_ethereum::EthereumNode;
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

// Custom Rlpx Subprotocol
pub mod proto {
    use super::*;
    use reth_eth_wire::capability::Capability;
    use reth_primitives::{Buf, BufMut};

    #[repr(u8)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum CustomRlpxProtoMessageId {
        Ping = 0x00,
        Pong = 0x01,
        CustomMessage = 0x02,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum CustomRlpxProtoMessageKind {
        Ping,
        Pong,
        CustomMessage(String),
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct CustomRlpxProtoMessage {
        pub message_type: CustomRlpxProtoMessageId,
        pub message: CustomRlpxProtoMessageKind,
    }

    impl CustomRlpxProtoMessage {
        /// Returns the capability for the `custom_rlpx` protocol.
        pub fn capability() -> Capability {
            Capability::new_static("custom_rlpx", 1)
        }

        /// Returns the protocol for the `custom_rlpx` protocol.
        pub fn protocol() -> Protocol {
            Protocol::new(Self::capability(), 3)
        }

        /// Creates a ping message
        pub fn ping() -> Self {
            Self {
                message_type: CustomRlpxProtoMessageId::Ping,
                message: CustomRlpxProtoMessageKind::Ping,
            }
        }

        /// Creates a pong message
        pub fn pong() -> Self {
            Self {
                message_type: CustomRlpxProtoMessageId::Pong,
                message: CustomRlpxProtoMessageKind::Pong,
            }
        }

        /// Creates a custom message
        pub fn custom_message(msg: impl Into<String>) -> Self {
            Self {
                message_type: CustomRlpxProtoMessageId::CustomMessage,
                message: CustomRlpxProtoMessageKind::CustomMessage(msg.into()),
            }
        }

        /// Creates a new `CustomRlpxProtoMessage` with the given message ID and payload.
        pub fn encoded(&self) -> BytesMut {
            let mut buf = BytesMut::new();
            buf.put_u8(self.message_type as u8);
            match &self.message {
                CustomRlpxProtoMessageKind::Ping => {}
                CustomRlpxProtoMessageKind::Pong => {}
                CustomRlpxProtoMessageKind::CustomMessage(msg) => {
                    buf.put(msg.as_bytes());
                }
            }
            buf
        }

        /// Decodes a `CustomRlpxProtoMessage` from the given message buffer.
        pub fn decode_message(buf: &mut &[u8]) -> Option<Self> {
            if buf.is_empty() {
                return None;
            }
            let id = buf[0];
            buf.advance(1);
            let message_type = match id {
                0x00 => CustomRlpxProtoMessageId::Ping,
                0x01 => CustomRlpxProtoMessageId::Pong,
                0x02 => CustomRlpxProtoMessageId::CustomMessage,
                _ => return None,
            };
            let message = match message_type {
                CustomRlpxProtoMessageId::Ping => CustomRlpxProtoMessageKind::Ping,
                CustomRlpxProtoMessageId::Pong => CustomRlpxProtoMessageKind::Pong,
                CustomRlpxProtoMessageId::CustomMessage => {
                    CustomRlpxProtoMessageKind::CustomMessage(
                        String::from_utf8_lossy(&buf[..]).into_owned(),
                    )
                }
            };
            Some(Self { message_type, message })
        }
    }
}

/// Custom Rlpx Subprotocol Handler
#[derive(Debug)]
struct CustomRlpxProtoHandler {
    state: ProtocolState,
}

impl ProtocolHandler for CustomRlpxProtoHandler {
    type ConnectionHandler = CustomRlpxConnectionHandler;

    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        Some(CustomRlpxConnectionHandler { state: self.state.clone() })
    }

    fn on_outgoing(
        &self,
        _socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        Some(CustomRlpxConnectionHandler { state: self.state.clone() })
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
    /// Send a custom message to the peer
    CustomMessage {
        msg: String,
        /// The response will be sent to this channel.
        response: oneshot::Sender<String>,
    },
}

struct CustomRlpxConnectionHandler {
    state: ProtocolState,
}

impl ConnectionHandler for CustomRlpxConnectionHandler {
    type Connection = CustomRlpxConnection;

    fn protocol(&self) -> Protocol {
        CustomRlpxProtoMessage::protocol()
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
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Self::Connection {
        let (tx, rx) = mpsc::unbounded_channel();
        self.state
            .events
            .send(ProtocolEvent::Established { direction, peer_id, to_connection: tx })
            .ok();
        CustomRlpxConnection {
            conn,
            initial_ping: direction.is_outgoing().then(CustomRlpxProtoMessage::ping),
            commands: UnboundedReceiverStream::new(rx),
            pending_pong: None,
        }
    }
}

struct CustomRlpxConnection {
    conn: ProtocolConnection,
    initial_ping: Option<CustomRlpxProtoMessage>,
    commands: UnboundedReceiverStream<Command>,
    pending_pong: Option<oneshot::Sender<String>>,
}

impl Stream for CustomRlpxConnection {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(initial_ping) = this.initial_ping.take() {
            return Poll::Ready(Some(initial_ping.encoded()));
        }

        loop {
            if let Poll::Ready(Some(cmd)) = this.commands.poll_next_unpin(cx) {
                return match cmd {
                    Command::CustomMessage { msg, response } => {
                        this.pending_pong = Some(response);
                        Poll::Ready(Some(CustomRlpxProtoMessage::ping().encoded()))
                    }
                };
            }
            let Some(msg) = ready!(this.conn.poll_next_unpin(cx)) else { return Poll::Ready(None) };
            let Some(msg) = CustomRlpxProtoMessage::decode_message(&mut &msg[..]) else {
                return Poll::Ready(None);
            };

            match msg.message {
                CustomRlpxProtoMessageKind::Ping => {
                    return Poll::Ready(Some(CustomRlpxProtoMessage::pong().encoded()))
                }
                CustomRlpxProtoMessageKind::Pong => {}
                CustomRlpxProtoMessageKind::CustomMessage(msg) => {
                    if let Some(sender) = this.pending_pong.take() {
                        sender.send(msg).ok();
                    }
                    continue;
                }
            }
            return Poll::Pending;
        }
    }
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, args| async move {
        // launch the node
        let NodeHandle { mut node, node_exit_future } =
            builder.node(EthereumNode::default()).launch().await?;
        

        
        // After lauch and after launch we inject a new rlpx protocol handler via the network node.network
        // the rlpx can be similar to the test example, could even be something like simple string message exchange
        
    

        // let custom_rlpx_handler = CustomRlpxProtoHandler{ state: ProtocolState { events: node.network.events.clone()}};
        // node.network.add_rlpx_sub_protocol(custom_rlpx_handler);

        // Spawn a task to handle incoming messages from the custom RLPx protocol
        
        node_exit_future.await
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use reth_tracing::init_test_tracing;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_custom_rlpx_proto() {
        
    }
}