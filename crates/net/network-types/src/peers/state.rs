//! State of connection to a peer.

/// Represents the kind of connection established to the peer, if any
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum PeerConnectionState {
    /// Not connected currently.
    #[default]
    Idle,
    /// Disconnect of an incoming connection in progress
    DisconnectingIn,
    /// Disconnect of an outgoing connection in progress
    DisconnectingOut,
    /// Connected via incoming connection.
    In,
    /// Connected via outgoing connection.
    Out,
    /// Pending outgoing connection.
    PendingOut,
}

// === impl PeerConnectionState ===

impl PeerConnectionState {
    /// Sets the disconnect state
    #[inline]
    pub fn disconnect(&mut self) {
        match self {
            Self::In => *self = Self::DisconnectingIn,
            Self::Out => *self = Self::DisconnectingOut,
            _ => {}
        }
    }

    /// Returns true if this is an active incoming connection.
    #[inline]
    pub const fn is_incoming(&self) -> bool {
        matches!(self, Self::In)
    }

    /// Returns whether we're currently connected with this peer
    #[inline]
    pub const fn is_connected(&self) -> bool {
        matches!(self, Self::In | Self::Out | Self::PendingOut)
    }

    /// Returns if there's currently no connection to that peer.
    #[inline]
    pub const fn is_unconnected(&self) -> bool {
        matches!(self, Self::Idle)
    }

    /// Returns true if there's currently an outbound dial to that peer.
    #[inline]
    pub const fn is_pending_out(&self) -> bool {
        matches!(self, Self::PendingOut)
    }
}
