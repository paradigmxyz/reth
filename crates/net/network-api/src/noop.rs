//! A network implementation that does nothing.
//!
//! This is useful for wiring components together that don't require network but still need to be
//! generic over it.

use core::{fmt, marker::PhantomData};
use std::net::{IpAddr, SocketAddr};

use alloy_rpc_types_admin::EthProtocolInfo;
use enr::{secp256k1::SecretKey, Enr};
use reth_eth_wire_types::{
    DisconnectReason, EthNetworkPrimitives, NetworkPrimitives, ProtocolVersion,
};
use reth_network_p2p::{sync::NetworkSyncUpdater, NoopFullBlockClient};
use reth_network_peers::NodeRecord;
use reth_network_types::{PeerKind, Reputation, ReputationChangeKind};
use reth_tokio_util::{EventSender, EventStream};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::{
    events::{NetworkPeersEvents, PeerEventStream},
    test_utils::{PeersHandle, PeersHandleProvider},
    BlockDownloaderProvider, DiscoveryEvent, NetworkError, NetworkEvent,
    NetworkEventListenerProvider, NetworkInfo, NetworkStatus, PeerId, PeerInfo, PeerRequest, Peers,
    PeersInfo,
};

/// A type that implements all network trait that does nothing.
///
/// Intended for testing purposes where network is not used.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NoopNetwork<Net = EthNetworkPrimitives> {
    peers_handle: PeersHandle,
    _marker: PhantomData<Net>,
}

impl<Net> NoopNetwork<Net> {
    /// Creates a new [`NoopNetwork`].
    pub fn new() -> Self {
        let (tx, _) = mpsc::unbounded_channel();

        Self { peers_handle: PeersHandle::new(tx), _marker: PhantomData }
    }
}

impl Default for NoopNetwork<EthNetworkPrimitives> {
    fn default() -> Self {
        let (tx, _) = mpsc::unbounded_channel();

        Self { peers_handle: PeersHandle::new(tx), _marker: PhantomData }
    }
}

impl<Net> NetworkInfo for NoopNetwork<Net>
where
    Net: Send + Sync,
{
    fn local_addr(&self) -> SocketAddr {
        (IpAddr::from(std::net::Ipv4Addr::UNSPECIFIED), 30303).into()
    }

    async fn network_status(&self) -> Result<NetworkStatus, NetworkError> {
        #[expect(deprecated)]
        Ok(NetworkStatus {
            client_version: "reth-test".to_string(),
            protocol_version: ProtocolVersion::V5 as u64,
            eth_protocol_info: EthProtocolInfo {
                network: 1,
                difficulty: None,
                genesis: Default::default(),
                config: Default::default(),
                head: Default::default(),
            },
        })
    }

    fn chain_id(&self) -> u64 {
        // mainnet
        1
    }

    fn is_syncing(&self) -> bool {
        false
    }

    fn is_initially_syncing(&self) -> bool {
        false
    }
}

impl<Net> PeersInfo for NoopNetwork<Net>
where
    Net: Send + Sync,
{
    fn num_connected_peers(&self) -> usize {
        0
    }

    fn local_node_record(&self) -> NodeRecord {
        NodeRecord::new(self.local_addr(), PeerId::random())
    }

    fn local_enr(&self) -> Enr<SecretKey> {
        let sk = SecretKey::from_slice(&[0xcd; 32]).unwrap();
        Enr::builder().build(&sk).unwrap()
    }
}

impl<Net> Peers for NoopNetwork<Net>
where
    Net: Send + Sync,
{
    fn add_trusted_peer_id(&self, _peer: PeerId) {}

    fn add_peer_kind(
        &self,
        _peer: PeerId,
        _kind: PeerKind,
        _tcp_addr: SocketAddr,
        _udp_addr: Option<SocketAddr>,
    ) {
    }

    async fn get_peers_by_kind(&self, _kind: PeerKind) -> Result<Vec<PeerInfo>, NetworkError> {
        Ok(vec![])
    }

    async fn get_all_peers(&self) -> Result<Vec<PeerInfo>, NetworkError> {
        Ok(vec![])
    }

    async fn get_peer_by_id(&self, _peer_id: PeerId) -> Result<Option<PeerInfo>, NetworkError> {
        Ok(None)
    }

    async fn get_peers_by_id(&self, _peer_id: Vec<PeerId>) -> Result<Vec<PeerInfo>, NetworkError> {
        Ok(vec![])
    }

    fn remove_peer(&self, _peer: PeerId, _kind: PeerKind) {}

    fn disconnect_peer(&self, _peer: PeerId) {}

    fn disconnect_peer_with_reason(&self, _peer: PeerId, _reason: DisconnectReason) {}

    fn connect_peer_kind(
        &self,
        _peer: PeerId,
        _kind: PeerKind,
        _tcp_addr: SocketAddr,
        _udp_addr: Option<SocketAddr>,
    ) {
    }

    fn reputation_change(&self, _peer_id: PeerId, _kind: ReputationChangeKind) {}

    async fn reputation_by_id(&self, _peer_id: PeerId) -> Result<Option<Reputation>, NetworkError> {
        Ok(None)
    }
}

impl<Net> BlockDownloaderProvider for NoopNetwork<Net>
where
    Net: NetworkPrimitives + Default,
{
    type Client = NoopFullBlockClient<Net>;

    async fn fetch_client(&self) -> Result<Self::Client, oneshot::error::RecvError> {
        Ok(NoopFullBlockClient::<Net>::default())
    }
}

impl<Net> NetworkSyncUpdater for NoopNetwork<Net>
where
    Net: fmt::Debug + Send + Sync + 'static,
{
    fn update_status(&self, _head: reth_ethereum_forks::Head) {}

    fn update_sync_state(&self, _state: reth_network_p2p::sync::SyncState) {}

    fn update_block_range(&self, _: reth_eth_wire_types::BlockRangeUpdate) {}
}

impl<Net> NetworkEventListenerProvider for NoopNetwork<Net>
where
    Net: NetworkPrimitives,
{
    type Primitives = Net;

    fn event_listener(&self) -> EventStream<NetworkEvent<PeerRequest<Self::Primitives>>> {
        let event_sender: EventSender<NetworkEvent<PeerRequest<Net>>> = Default::default();
        event_sender.new_listener()
    }

    fn discovery_listener(&self) -> UnboundedReceiverStream<DiscoveryEvent> {
        let (_, rx) = mpsc::unbounded_channel();
        UnboundedReceiverStream::new(rx)
    }
}

impl<Net> NetworkPeersEvents for NoopNetwork<Net>
where
    Net: NetworkPrimitives,
{
    fn peer_events(&self) -> PeerEventStream {
        let event_sender: EventSender<NetworkEvent<PeerRequest<Net>>> = Default::default();
        PeerEventStream::new(event_sender.new_listener())
    }
}

impl<Net> PeersHandleProvider for NoopNetwork<Net>
where
    Net: NetworkPrimitives,
{
    fn peers_handle(&self) -> &PeersHandle {
        &self.peers_handle
    }
}
