//! Coordinates EIP-8189 state bootstrap without changing Reth's default sync path.
//!
//! Verified ranges are persisted as resumable v2 hashed-state generations before BAL catch-up and
//! final trie validation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod catch_up;
mod download;
mod error;
mod store;
mod trie;

pub use catch_up::{BlockAccessListCatchUp, BlockAccessListCatchUpOutcome};
pub use download::{StateDownloadOutcome, StateDownloader};
pub use error::SnapSyncError;
pub use store::{
    AccountRangeProgress, BlockAccessListProgress, SnapGeneration, SnapPhase, SnapStateStore,
};
pub use trie::TrieGenerator;
