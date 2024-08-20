//! Commonly used networking types.
//!
//! ## Feature Flags
//!
//! - `serde` (default): Enable serde support

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Types related to peering.
pub mod peers;
pub mod session;

/// [`BackoffKind`] definition.
mod backoff;

pub use peers::reputation::{Reputation, ReputationChangeKind, ReputationChangeWeights};

pub use backoff::BackoffKind;
pub use peers::{
    addr::PeerAddr,
    kind::PeerKind,
    reputation::{is_banned_reputation, ReputationChangeOutcome, DEFAULT_REPUTATION},
    state::PeerConnectionState,
    ConnectionsConfig, Peer, PeersConfig,
};
pub use session::{SessionLimits, SessionsConfig};
