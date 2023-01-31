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
    /// Peer sent a bad transaction messages. E.g. Transactions which weren't recoverable.
    BadTransactions,
    /// Peer failed to respond in time.
    Timeout,
    /// Peer does not adhere to network protocol rules.
    BadProtocol,
    /// Failed to establish a connection to the peer.
    FailedToConnect,
    /// Connection dropped by peer.
    Dropped,
    /// Apply a reputation change by value
    Other(Reputation),
}
