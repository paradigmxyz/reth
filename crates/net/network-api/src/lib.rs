#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxzy/reth/issues/"
)]
#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth network interface definitions.
//!
//! Provides abstractions for the reth-network crate.
//!
//! ## Feature Flags
//!
//! - `serde` (default): Enable serde support
use async_trait::async_trait;
use reth_eth_wire::{DisconnectReason, EthVersion, Status};
use reth_primitives::{NodeRecord, PeerId};
use reth_rpc_types::NetworkStatus;
use std::{net::SocketAddr, sync::Arc};

pub use error::NetworkError;
pub use reputation::{Reputation, ReputationChangeKind};
use reth_eth_wire::capability::Capabilities;

/// Network Error
pub mod error;
/// Reputation score
pub mod reputation;

/// Implementation of network traits for that does nothing.
pub mod noop;

/// Provides general purpose information about the network.
#[async_trait]
pub trait NetworkInfo: Send + Sync {
    /// Returns the [`SocketAddr`] that listens for incoming connections.
    fn local_addr(&self) -> SocketAddr;

    /// Returns the current status of the network being ran by the local node.
    async fn network_status(&self) -> Result<NetworkStatus, NetworkError>;

    /// Returns the chain id
    fn chain_id(&self) -> u64;

    /// Returns `true` if the network is undergoing sync.
    fn is_syncing(&self) -> bool;

    /// Returns `true` when the node is undergoing the very first Pipeline sync.
    fn is_initially_syncing(&self) -> bool;
}

/// Provides general purpose information about Peers in the network.
pub trait PeersInfo: Send + Sync {
    /// Returns how many peers the network is currently connected to.
    ///
    /// Note: this should only include established connections and _not_ ongoing attempts.
    fn num_connected_peers(&self) -> usize;

    /// Returns the Ethereum Node Record of the node.
    fn local_node_record(&self) -> NodeRecord;
}

/// Provides an API for managing the peers of the network.
#[async_trait]
pub trait Peers: PeersInfo {
    /// Adds a peer to the peer set.
    fn add_peer(&self, peer: PeerId, addr: SocketAddr) {
        self.add_peer_kind(peer, PeerKind::Basic, addr);
    }

    /// Adds a trusted peer to the peer set.
    fn add_trusted_peer(&self, peer: PeerId, addr: SocketAddr) {
        self.add_peer_kind(peer, PeerKind::Trusted, addr);
    }

    /// Adds a peer to the known peer set, with the given kind.
    fn add_peer_kind(&self, peer: PeerId, kind: PeerKind, addr: SocketAddr);

    /// Returns the rpc [PeerInfo] for all connected peers.
    async fn get_peers(&self) -> Result<Vec<PeerInfo>, NetworkError>;

    /// Removes a peer from the peer set that corresponds to given kind.
    fn remove_peer(&self, peer: PeerId, kind: PeerKind);

    /// Disconnect an existing connection to the given peer.
    fn disconnect_peer(&self, peer: PeerId);

    /// Disconnect an existing connection to the given peer using the provided reason
    fn disconnect_peer_with_reason(&self, peer: PeerId, reason: DisconnectReason);

    /// Send a reputation change for the given peer.
    fn reputation_change(&self, peer_id: PeerId, kind: ReputationChangeKind);

    /// Get the reputation of a peer.
    async fn reputation_by_id(&self, peer_id: PeerId) -> Result<Option<Reputation>, NetworkError>;
}

/// Represents the kind of peer
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum PeerKind {
    /// Basic peer kind.
    #[default]
    Basic,
    /// Trusted peer.
    Trusted,
}

/// Info about an active peer session.
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// Announced capabilities of the peer
    pub capabilities: Arc<Capabilities>,
    /// The identifier of the remote peer
    pub remote_id: PeerId,
    /// The client's name and version
    pub client_version: Arc<String>,
    /// The peer's address we're connected to
    pub remote_addr: SocketAddr,
    /// The local address of the connection
    pub local_addr: Option<SocketAddr>,
    /// The direction of the session
    pub direction: Direction,
    /// The negotiated eth version.
    pub eth_version: EthVersion,
    /// The Status message the peer sent for the `eth` handshake
    pub status: Status,
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
    pub fn is_incoming(&self) -> bool {
        matches!(self, Direction::Incoming)
    }

    /// Returns `true` if this an outgoing connection.
    pub fn is_outgoing(&self) -> bool {
        matches!(self, Direction::Outgoing(_))
    }
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::Incoming => write!(f, "incoming"),
            Direction::Outgoing(_) => write!(f, "outgoing"),
        }
    }
}
