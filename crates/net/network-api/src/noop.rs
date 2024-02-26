//! A network implementation that does nothing.
//!
//! This is useful for wiring components together that don't require network but still need to be
//! generic over it.

use crate::{
    NetworkError, NetworkInfo, PeerInfo, PeerKind, Peers, PeersInfo, Reputation,
    ReputationChangeKind,
};
use alloy_chains::Chain;
use reth_discv4::DEFAULT_DISCOVERY_PORT;
use reth_eth_wire::{DisconnectReason, ProtocolVersion};
use reth_primitives::{NodeRecord, PeerId};
use reth_rpc_types::{EthProtocolInfo, NetworkStatus};
use std::net::{IpAddr, SocketAddr};

/// A type that implements all network trait that does nothing.
///
/// Intended for testing purposes where network is not used.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct NoopNetwork;

impl NetworkInfo for NoopNetwork {
    fn local_addr(&self) -> SocketAddr {
        (IpAddr::from(std::net::Ipv4Addr::UNSPECIFIED), DEFAULT_DISCOVERY_PORT).into()
    }

    async fn network_status(&self) -> Result<NetworkStatus, NetworkError> {
        Ok(NetworkStatus {
            client_version: "reth-test".to_string(),
            protocol_version: ProtocolVersion::V5 as u64,
            eth_protocol_info: EthProtocolInfo {
                difficulty: Default::default(),
                head: Default::default(),
                network: 1,
                genesis: Default::default(),
            },
        })
    }

    fn chain_id(&self) -> u64 {
        Chain::mainnet().into()
    }

    fn is_syncing(&self) -> bool {
        false
    }

    fn is_initially_syncing(&self) -> bool {
        false
    }

    #[cfg(feature = "optimism")]
    fn sequencer_endpoint(&self) -> Option<&str> {
        None
    }
}

impl PeersInfo for NoopNetwork {
    fn num_connected_peers(&self) -> usize {
        0
    }

    fn local_node_record(&self) -> NodeRecord {
        NodeRecord::new(self.local_addr(), PeerId::random())
    }
}

impl Peers for NoopNetwork {
    fn add_peer_kind(&self, _peer: PeerId, _kind: PeerKind, _addr: SocketAddr) {}

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

    fn reputation_change(&self, _peer_id: PeerId, _kind: ReputationChangeKind) {}

    async fn reputation_by_id(&self, _peer_id: PeerId) -> Result<Option<Reputation>, NetworkError> {
        Ok(None)
    }
}
