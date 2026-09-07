//! snap/2 state synchronization for [EIP-8189](https://eips.ethereum.org/EIPS/eip-8189).
//!
//! Coordinates a state bootstrap that starts from a recent pivot block, downloads accounts, storage
//! and bytecode authenticated against that pivot's state root, and advances the pivot with
//! [EIP-7928 block access lists](https://eips.ethereum.org/EIPS/eip-7928) as the chain moves past
//! it.
//!
//! This crate owns download progress only. Authenticated downloads come from
//! `reth-downloaders`, and verified state is handed back to node integration once its trie root
//! matches the target header.
//!
//! ```
//! use reth_snap_sync::SnapPivotPolicy;
//!
//! let policy = SnapPivotPolicy::default();
//! // Without a finalized block, anchor at the EIP's example distance.
//! assert_eq!(policy.pivot_block(1_000, None), Some(936));
//! // A recent finalized block is anchored to directly.
//! assert_eq!(policy.pivot_block(1_000, Some(950)), Some(950));
//! // Stalled finality falls back to the example distance.
//! assert_eq!(policy.pivot_block(1_000, Some(500)), Some(936));
//! // A chain shorter than the head distance has no pivot yet.
//! assert_eq!(policy.pivot_block(4, None), None);
//! ```

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod error;
mod generation;
mod pivot;
mod session;

#[cfg(test)]
mod test_utils;

pub use error::SnapSyncError;
pub use generation::{SnapGeneration, SnapPhase};
pub use pivot::SnapPivotPolicy;
pub use session::{SnapSyncSession, SnapSyncSessionState};
