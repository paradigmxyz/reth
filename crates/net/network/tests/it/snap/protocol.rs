//! A minimal, inert `RLPx` satellite sub-protocol for tests that need a peer to negotiate an
//! extra capability without any real protocol behavior.

use alloy_primitives::bytes::BytesMut;
use futures::Stream;
use reth_eth_wire::{
    capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol, Capability,
};
use reth_network::{
    config::rng_secret_key,
    error::NetworkError,
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

/// A [`ProtocolHandler`] that negotiates `protocol` but never sends or expects any messages.
#[derive(Debug, Clone)]
pub(super) struct InertProtocolHandler(Protocol);

impl InertProtocolHandler {
    /// Creates a handler for `protocol`.
    pub(super) const fn new(protocol: Protocol) -> Self {
        Self(protocol)
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
    for snap_first in [false, true] {
        let builder = NetworkConfig::builder(rng_secret_key(), Runtime::test());
        let handler = InertProtocolHandler::new(Protocol::new(Capability::new_static("les", 1), 1));
        let builder = if snap_first {
            builder.with_snap(true).add_rlpx_sub_protocol(handler)
        } else {
            builder.add_rlpx_sub_protocol(handler).with_snap(true)
        };
        let result = NetworkManager::eth(builder.build(MockEthProvider::default())).await;
        assert!(matches!(result, Err(NetworkError::SnapWithExtraProtocols)));
        assert_eq!(
            result.err().unwrap().to_string(),
            "snap/2 does not support additional RLPx subprotocols; disable snap/2 or remove the additional protocols",
        );
    }
}

#[tokio::test]
async fn snap_rejects_extra_protocols_in_custom_hello() {
    let mut config = NetworkConfig::builder(rng_secret_key(), Runtime::test())
        .with_snap(true)
        .build(MockEthProvider::default());
    config
        .hello_message
        .try_add_protocol(Protocol::new(Capability::new_static("les", 1), 1))
        .unwrap();
    assert!(
        matches!(NetworkManager::eth(config).await, Err(NetworkError::SnapWithExtraProtocols),)
    );
}

#[tokio::test]
async fn extra_protocol_registration_requires_snap_disabled() {
    for snap_enabled in [false, true] {
        let config = NetworkConfig::builder(rng_secret_key(), Runtime::test())
            .listener_addr("127.0.0.1:0".parse().unwrap())
            .disable_discovery()
            .with_snap(snap_enabled)
            .build(MockEthProvider::default());
        let mut network = NetworkManager::eth(config).await.unwrap();
        let handler = InertProtocolHandler::new(Protocol::new(Capability::new_static("les", 1), 1));
        for result in [
            network.add_rlpx_sub_protocol(handler.clone()),
            network.handle().add_rlpx_sub_protocol(handler.into_rlpx_sub_protocol()),
        ] {
            if snap_enabled {
                assert!(matches!(result, Err(NetworkError::SnapWithExtraProtocols)));
            } else {
                result.unwrap();
            }
        }
    }
}
