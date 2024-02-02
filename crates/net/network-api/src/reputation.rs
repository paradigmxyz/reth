/// The type that tracks the reputation score.
pub type Reputation = i32;

/// Various kinds of reputation changes.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ReputationChangeKind {
    /// Received an unspecific bad message from the peer
    BadMessage,
    /// Peer sent a bad block.
    ///
    /// Note: this will we only used in pre-merge, pow consensus, since after no more block announcements are sent via devp2p: [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)
    BadBlock,
    /// Peer sent a bad transaction message. E.g. Transactions which weren't recoverable.
    BadTransactions,
    /// Peer sent a bad announcement message, e.g. invalid transaction type for the configured
    /// network.
    BadAnnouncement,
    /// Peer sent a message that included a hash or transaction that we already received from the
    /// peer.
    ///
    /// According to the [Eth spec](https://github.com/ethereum/devp2p/blob/master/caps/eth.md):
    ///
    /// > A node should never send a transaction back to a peer that it can determine already knows
    /// > of it (either because it was previously sent or because it was informed from this peer
    /// > originally). This is usually achieved by remembering a set of transaction hashes recently
    /// > relayed by the peer.
    AlreadySeenTransaction,
    /// Peer failed to respond in time.
    Timeout,
    /// Peer does not adhere to network protocol rules.
    BadProtocol,
    /// Failed to establish a connection to the peer.
    FailedToConnect,
    /// Connection dropped by peer.
    Dropped,
    /// Reset the reputation to the default value.
    Reset,
    /// Apply a reputation change by value
    Other(Reputation),
}

impl ReputationChangeKind {
    /// Returns true if the reputation change is a reset.
    pub fn is_reset(&self) -> bool {
        matches!(self, Self::Reset)
    }
}
