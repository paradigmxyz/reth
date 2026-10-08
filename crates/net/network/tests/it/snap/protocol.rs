//! Tests that `snap/2` rejects additional `RLPx` sub-protocols, using an inert `les/1` handler.

use alloy_primitives::bytes::BytesMut;
use futures::Stream;
use reth_eth_wire::{
    capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol, Capability,
};
use reth_network::{
    config::rng_secret_key,
    protocol::{ConnectionHandler, IntoRlpxSubProtocol, OnNotSupported, ProtocolHandler},
    NetworkConfig, NetworkManager, NetworkProtocols,
};
use reth_network_api::{Direction, PeerId};
use reth_provider::test_utils::MockEthProvider;
use reth_tasks::Runtime;
use std::{
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};

// Error message returned when `snap/2` is combined with additional `RLPx` protocols.
const SNAP_WITH_EXTRA_PROTOCOLS: &str = "snap/2 does not support additional RLPx subprotocols; disable snap/2 or remove the additional protocols";

/// A [`ProtocolHandler`] that negotiates `protocol` but never sends or expects any messages.
#[derive(Debug, Clone)]
struct InertProtocolHandler(Protocol);

impl InertProtocolHandler {
    /// Creates a handler for `les/1`.
    const fn les() -> Self {
        Self(Protocol::new(Capability::new_static("les", 1), 1))
    }
}

impl ProtocolHandler for InertProtocolHandler {
    type ConnectionHandler = Self;

    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        Some(self.clone())
    }

    fn on_outgoing(
        &self,
        _socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        Some(self.clone())
    }
}

impl ConnectionHandler for InertProtocolHandler {
    type Connection = InertConnection;

    fn protocol(&self) -> Protocol {
        self.0.clone()
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
        _direction: Direction,
        _peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Self::Connection {
        InertConnection(conn)
    }
}

/// The connection for [`InertProtocolHandler`]. Just forwards whatever the remote sends.
pub(super) struct InertConnection(ProtocolConnection);

impl Stream for InertConnection {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.get_mut().0).poll_next(cx)
    }
}

#[tokio::test]
async fn snap_rejects_configured_extra_protocols() {
    let config = NetworkConfig::builder(rng_secret_key(), Runtime::test())
        .with_snap(true)
        .add_rlpx_sub_protocol(InertProtocolHandler::les())
        .build(MockEthProvider::default());

    let err = NetworkManager::eth(config).await.unwrap_err();
    assert_eq!(err.to_string(), SNAP_WITH_EXTRA_PROTOCOLS);
}

#[tokio::test]
async fn snap_rejects_extra_protocols_in_custom_hello() {
    let mut config = NetworkConfig::builder(rng_secret_key(), Runtime::test())
        .with_snap(true)
        .build(MockEthProvider::default());
    config.hello_message.try_add_protocol(InertProtocolHandler::les().0).unwrap();

    let err = NetworkManager::eth(config).await.unwrap_err();
    assert_eq!(err.to_string(), SNAP_WITH_EXTRA_PROTOCOLS);
}

#[tokio::test]
async fn snap_rejects_extra_protocol_registration() {
    let config = NetworkConfig::builder(rng_secret_key(), Runtime::test())
        .listener_addr("127.0.0.1:0".parse().unwrap())
        .disable_discovery()
        .with_snap(true)
        .build(MockEthProvider::default());
    let mut network = NetworkManager::eth(config).await.unwrap();

    let err = network.add_rlpx_sub_protocol(InertProtocolHandler::les()).unwrap_err();
    assert_eq!(err.to_string(), SNAP_WITH_EXTRA_PROTOCOLS);
    let err = network
        .handle()
        .add_rlpx_sub_protocol(InertProtocolHandler::les().into_rlpx_sub_protocol())
        .unwrap_err();
    assert_eq!(err.to_string(), SNAP_WITH_EXTRA_PROTOCOLS);
}
