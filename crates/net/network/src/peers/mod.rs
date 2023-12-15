//! Peer related implementations

mod manager;
mod reputation;

pub(crate) use manager::InboundConnectionError;
pub use manager::{ConnectionInfo, Peer, PeerAction, PeersConfig, PeersHandle, PeersManager};
pub use reputation::ReputationChangeWeights;
pub use reth_network_api::PeerKind;

/// Maximum number of available slots for outbound sessions.
pub(crate) const DEFAULT_MAX_PEERS_OUTBOUND: usize = 100;

/// Maximum number of available slots for inbound sessions.
pub(crate) const DEFAULT_MAX_PEERS_INBOUND: usize = 30;

/// Maximum number of available slots concurrent outgoing dials.
pub(crate) const DEFAULT_MAX_CONCURRENT_DIALS: usize = 10;
