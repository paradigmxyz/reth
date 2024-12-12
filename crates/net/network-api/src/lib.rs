//! Reth interface definitions and commonly used types for the reth-network crate.
//!
//! Provides abstractions for the reth-network crate.
//!
//! ## Feature Flags
//!
//! - `serde` (default): Enable serde support

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod downloaders;
/// Network Error
pub mod error;
pub mod events;
/// Implementation of network traits for that does nothing.
pub mod noop;
pub mod test_utils;

pub use alloy_rpc_types_admin::EthProtocolInfo;
use reth_network_p2p::sync::NetworkSyncUpdater;
pub use reth_network_p2p::BlockClient;
pub use reth_network_types::{PeerKind, Reputation, ReputationChangeKind};

pub use downloaders::BlockDownloaderProvider;
pub use error::NetworkError;
pub use events::{
    DiscoveredEvent, DiscoveryEvent, NetworkEvent, NetworkEventListenerProvider, PeerRequest,
    PeerRequestSender,
};

use std::{future::Future, net::SocketAddr, sync::Arc, time::Instant};

use reth_eth_wire_types::{capability::Capabilities, DisconnectReason, EthVersion, Status};
use reth_network_peers::NodeRecord;

/// The `PeerId` type.
pub type PeerId = alloy_primitives::B512;

/// Helper trait that unifies network API needed to launch node.
pub trait FullNetwork:
    BlockDownloaderProvider
    + NetworkSyncUpdater
    + NetworkInfo
    + NetworkEventListenerProvider
    + PeersInfo
    + Peers
    + Clone
    + 'static
{
}

impl<T> FullNetwork for T where
    T: BlockDownloaderProvider
        + NetworkSyncUpdater
        + NetworkInfo
        + NetworkEventListenerProvider
        + PeersInfo
        + Peers
        + Clone
        + 'static
{
}

/// Provides general purpose information about the network.
#[auto_impl::auto_impl(&, Arc)]
pub trait NetworkInfo: Send + Sync {
    /// Returns the [`SocketAddr`] that listens for incoming connections.
    fn local_addr(&self) -> SocketAddr;

    /// Returns the current status of the network being ran by the local node.
    fn network_status(&self) -> impl Future<Output = Result<NetworkStatus, NetworkError>> + Send;

    /// Returns the chain id
    fn chain_id(&self) -> u64;

    /// Returns `true` if the network is undergoing sync.
    fn is_syncing(&self) -> bool;

    /// Returns `true` when the node is undergoing the very first Pipeline sync.
    fn is_initially_syncing(&self) -> bool;
}

/// Provides general purpose information about Peers in the network.
#[auto_impl::auto_impl(&, Arc)]
pub trait PeersInfo: Send + Sync {
    /// Returns how many peers the network is currently connected to.
    ///
    /// Note: this should only include established connections and _not_ ongoing attempts.
    fn num_connected_peers(&self) -> usize;

    /// Returns the Ethereum Node Record of the node.
    fn local_node_record(&self) -> NodeRecord;

    /// Returns the local ENR of the node.
    fn local_enr(&self) -> enr::Enr<enr::secp256k1::SecretKey>;
}

/// Provides an API for managing the peers of the network.
#[auto_impl::auto_impl(&, Arc)]
pub trait Peers: PeersInfo {
    /// Adds a peer to the peer set with TCP `SocketAddr`.
    fn add_peer(&self, peer: PeerId, tcp_addr: SocketAddr) {
        self.add_peer_kind(peer, PeerKind::Static, tcp_addr, None);
    }

    /// Adds a peer to the peer set with TCP and UDP `SocketAddr`.
    fn add_peer_with_udp(&self, peer: PeerId, tcp_addr: SocketAddr, udp_addr: SocketAddr) {
        self.add_peer_kind(peer, PeerKind::Static, tcp_addr, Some(udp_addr));
    }

    /// Adds a trusted [`PeerId`] to the peer set.
    ///
    /// This allows marking a peer as trusted without having to know the peer's address.
    fn add_trusted_peer_id(&self, peer: PeerId);

    /// Adds a trusted peer to the peer set with TCP `SocketAddr`.
    fn add_trusted_peer(&self, peer: PeerId, tcp_addr: SocketAddr) {
        self.add_peer_kind(peer, PeerKind::Trusted, tcp_addr, None);
    }

    /// Adds a trusted peer with TCP and UDP `SocketAddr` to the peer set.
    fn add_trusted_peer_with_udp(&self, peer: PeerId, tcp_addr: SocketAddr, udp_addr: SocketAddr) {
        self.add_peer_kind(peer, PeerKind::Trusted, tcp_addr, Some(udp_addr));
    }

    /// Adds a peer to the known peer set, with the given kind.
    fn add_peer_kind(
        &self,
        peer: PeerId,
        kind: PeerKind,
        tcp_addr: SocketAddr,
        udp_addr: Option<SocketAddr>,
    );

    /// Returns the rpc [`PeerInfo`] for all connected [`PeerKind::Trusted`] peers.
    fn get_trusted_peers(
        &self,
    ) -> impl Future<Output = Result<Vec<PeerInfo>, NetworkError>> + Send {
        self.get_peers_by_kind(PeerKind::Trusted)
    }

    /// Returns the rpc [`PeerInfo`] for all connected [`PeerKind::Basic`] peers.
    fn get_basic_peers(&self) -> impl Future<Output = Result<Vec<PeerInfo>, NetworkError>> + Send {
        self.get_peers_by_kind(PeerKind::Basic)
    }

    /// Returns the rpc [`PeerInfo`] for all connected peers with the given kind.
    fn get_peers_by_kind(
        &self,
        kind: PeerKind,
    ) -> impl Future<Output = Result<Vec<PeerInfo>, NetworkError>> + Send;

    /// Returns the rpc [`PeerInfo`] for all connected peers.
    fn get_all_peers(&self) -> impl Future<Output = Result<Vec<PeerInfo>, NetworkError>> + Send;

    /// Returns the rpc [`PeerInfo`] for the given peer id.
    ///
    /// Returns `None` if the peer is not connected.
    fn get_peer_by_id(
        &self,
        peer_id: PeerId,
    ) -> impl Future<Output = Result<Option<PeerInfo>, NetworkError>> + Send;

    /// Returns the rpc [`PeerInfo`] for the given peers if they are connected.
    ///
    /// Note: This only returns peers that are connected, unconnected peers are ignored but keeping
    /// the order in which they were requested.
    fn get_peers_by_id(
        &self,
        peer_ids: Vec<PeerId>,
    ) -> impl Future<Output = Result<Vec<PeerInfo>, NetworkError>> + Send;

    /// Removes a peer from the peer set that corresponds to given kind.
    fn remove_peer(&self, peer: PeerId, kind: PeerKind);

    /// Disconnect an existing connection to the given peer.
    fn disconnect_peer(&self, peer: PeerId);

    /// Disconnect an existing connection to the given peer using the provided reason
    fn disconnect_peer_with_reason(&self, peer: PeerId, reason: DisconnectReason);

    /// Connect to the given peer. NOTE: if the maximum number out outbound sessions is reached,
    /// this won't do anything. See `reth_network::SessionManager::dial_outbound`.
    fn connect_peer(&self, peer: PeerId, tcp_addr: SocketAddr) {
        self.connect_peer_kind(peer, PeerKind::Static, tcp_addr, None)
    }

    /// Connects a peer to the known peer set, with the given kind.
    fn connect_peer_kind(
        &self,
        peer: PeerId,
        kind: PeerKind,
        tcp_addr: SocketAddr,
        udp_addr: Option<SocketAddr>,
    );

    /// Send a reputation change for the given peer.
    fn reputation_change(&self, peer_id: PeerId, kind: ReputationChangeKind);

    /// Get the reputation of a peer.
    fn reputation_by_id(
        &self,
        peer_id: PeerId,
    ) -> impl Future<Output = Result<Option<Reputation>, NetworkError>> + Send;
}

/// Info about an active peer session.
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// Announced capabilities of the peer
    pub capabilities: Arc<Capabilities>,
    /// The identifier of the remote peer
    pub remote_id: PeerId,
    /// The client's name and version
    pub client_version: Arc<str>,
    /// The peer's enode
    pub enode: String,
    /// The peer's enr
    pub enr: Option<String>,
    /// The peer's address we're connected to
    pub remote_addr: SocketAddr,
    /// The local address of the connection
    pub local_addr: Option<SocketAddr>,
    /// The direction of the session
    pub direction: Direction,
    /// The negotiated eth version.
    pub eth_version: EthVersion,
    /// The Status message the peer sent for the `eth` handshake
    pub status: Arc<Status>,
    /// The timestamp when the session to that peer has been established.
    pub session_established: Instant,
    /// The peer's connection kind
    pub kind: PeerKind,
}

/// The direction of the connection.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Direction {
    /// Incoming connection.
    Incoming,
    /// Outgoing connection to a specific node.
    Outgoing(PeerId),
}

impl Direction {
    /// Returns `true` if this an incoming connection.
    pub const fn is_incoming(&self) -> bool {
        matches!(self, Self::Incoming)
    }

    /// Returns `true` if this an outgoing connection.
    pub const fn is_outgoing(&self) -> bool {
        matches!(self, Self::Outgoing(_))
    }
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Incoming => write!(f, "incoming"),
            Self::Outgoing(_) => write!(f, "outgoing"),
        }
    }
}

/// The status of the network being ran by the local node.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NetworkStatus {
    /// The local node client version.
    pub client_version: String,
    /// The current ethereum protocol version
    pub protocol_version: u64,
    /// Information about the Ethereum Wire Protocol.
    pub eth_protocol_info: EthProtocolInfo,
}
