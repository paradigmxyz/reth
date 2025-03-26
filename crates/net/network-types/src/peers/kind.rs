//! Classification of a peer based on trust.

/// Represents the kind of peer
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum PeerKind {
    /// Basic peer kind.
    #[default]
    Basic,
    /// Static peer, added via JSON-RPC.
    Static,
    /// Trusted peer.
    Trusted,
}

impl PeerKind {
    /// Returns `true` if the peer is trusted.
    pub const fn is_trusted(&self) -> bool {
        matches!(self, Self::Trusted)
    }

    /// Returns `true` if the peer is static.
    pub const fn is_static(&self) -> bool {
        matches!(self, Self::Static)
    }

    /// Returns `true` if the peer is basic.
    pub const fn is_basic(&self) -> bool {
        matches!(self, Self::Basic)
    }
}
