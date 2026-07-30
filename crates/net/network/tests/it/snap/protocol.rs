//! A minimal, inert `RLPx` satellite sub-protocol for tests that need a peer to negotiate an
//! extra capability without any real protocol behavior.

use alloy_primitives::{bytes::BytesMut, B256};
use futures::Stream;
use reth_eth_wire::{
    capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol,
    snap::GetAccountRangeMessage, Capability, EthVersion,
};
use reth_network::{
    eth_requests::SOFT_RESPONSE_LIMIT,
    protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler},
    test_utils::{PeerConfig, Testnet},
    BlockDownloaderProvider,
};
use reth_network_api::{Direction, PeerId};
use reth_network_p2p::{error::RequestError, snap::client::SnapClient};
use reth_provider::test_utils::MockEthProvider;
use reth_transaction_pool::test_utils::TestPool;
use std::{
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
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

#[tokio::test(flavor = "multi_thread")]
async fn eth_snap_and_third_satellite_protocol_fails_fast_on_snap_request() {
    reth_tracing::init_test_tracing();

    // Snap is only wired up for the dedicated eth+snap/2 connection; a third negotiated
    // capability forces the satellite multiplexer instead, which does not serve snap. A snap
    // request over such a session must fail fast with a typed error rather than hang waiting for
    // a response that will never come.
    let les_protocol = Protocol::new(Capability::new_static("les", 1), 1);
    let protocols = vec![EthVersion::Eth71.into(), Protocol::snap_2(), les_protocol.clone()];

    let provider = Arc::new(MockEthProvider::default());
    let mut net: Testnet<_, TestPool> = Testnet::default();
    for _ in 0..2 {
        let peer = PeerConfig::with_protocols(provider.clone(), protocols.clone());
        net.add_peer_with_config(peer).await.unwrap();
    }
    net.for_each_mut(|peer| {
        peer.install_request_handler();
        peer.add_rlpx_sub_protocol(InertProtocolHandler::new(les_protocol.clone()));
    });
    let net = net.spawn();
    net.connect_peers().await;
    let fetch = net.peers()[0].network().fetch_client().await.unwrap();

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        fetch.get_account_range(GetAccountRangeMessage {
            request_id: 51,
            root_hash: B256::ZERO,
            starting_hash: B256::ZERO,
            limit_hash: B256::repeat_byte(0xff),
            response_bytes: SOFT_RESPONSE_LIMIT as u64,
        }),
    )
    .await
    .expect("request should not hang");

    assert_eq!(result.unwrap_err(), RequestError::UnsupportedCapability);
}
