//! Errors interfacing with [`discv5::Discv5`].

use discv5::IpMode;

/// Errors interfacing with [`discv5::Discv5`].
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Failure adding node to [`discv5::Discv5`].
    #[error("failed adding node to discv5, {0}")]
    AddNodeFailed(&'static str),
    /// Node record has incompatible key type.
    #[error("incompatible key type (not secp256k1)")]
    IncompatibleKeyType,
    /// No key used to identify rlpx network is configured.
    #[error("network stack identifier is not configured")]
    NetworkStackIdNotConfigured,
    /// Missing key used to identify rlpx network.
    #[error("fork missing on enr, key missing")]
    ForkMissing(&'static [u8]),
    /// Failed to decode [`ForkId`](reth_primitives::ForkId) rlp value.
    #[error("failed to decode fork id, 'eth': {0:?}")]
    ForkIdDecodeError(#[from] alloy_rlp::Error),
    /// Peer is unreachable over discovery.
    #[error("discovery socket missing")]
    UnreachableDiscovery,
    /// Peer is unreachable over rlpx.
    #[error("RLPx TCP socket missing")]
    UnreachableRlpx,
    /// Peer is not using same IP version as local node in rlpx.
    #[error("RLPx TCP socket is unsupported IP version, local ip mode: {0:?}")]
    IpVersionMismatchRlpx(IpMode),
    /// Failed to initialize [`discv5::Discv5`].
    #[error("init failed, {0}")]
    InitFailure(&'static str),
    /// An error from underlying [`discv5::Discv5`] node.
    #[error("sigp/discv5 error, {0}")]
    Discv5Error(discv5::Error),
    /// The [`ListenConfig`](discv5::ListenConfig) has been misconfigured.
    #[error("misconfigured listen config, RLPx TCP address must also be supported by discv5")]
    ListenConfigMisconfigured,
}
