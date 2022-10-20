//! High level network management.
//!
//! The [`Network`] contains the state of the network as a whole. It controls how connections are
//! handled and keeps track of connections to peers.

use crate::{
    connections::Connections, discovery::Discovery, listener::ConnectionListener, peers::PeerSet,
};

/// Manages the entire state of the network.
pub struct Network {
    /// Listens for new incoming connections.
    incoming: ConnectionListener,
    /// Active connections
    connections: Connections,
    /// Ethereum peer discovery related network IO.
    discover: Discovery,
    /// Manages lists of peers and address to reject.
    peer_set: PeerSet,
}
