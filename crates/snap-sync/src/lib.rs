//! snap/2 state synchronization for [EIP-8189](https://eips.ethereum.org/EIPS/eip-8189).
//! Downloads accounts, storage and bytecode authenticated against a canonical pivot.
//! [EIP-7928 block access lists](https://eips.ethereum.org/EIPS/eip-7928) advance the downloaded state.
//! Domain modules separate downloads from persistence; progress commits with its state.
//! The rebuilt trie must match the target header before handing state to the pipeline.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod account;
mod attempt;
mod bal;
mod bootstrap;
mod bytecode;
mod catch_up;
mod context;
mod error;
mod generation;
mod handoff;
mod pivot;
mod request;
mod session;
mod storage;
mod trie;

#[cfg(test)]
mod test_utils;

pub use account::{
    AccountCoverage, AccountRangeDownload, AccountRangeProgress, AccountRangeStep,
    SnapAccountStore, VerifiedRange, DEFAULT_RESPONSE_BYTES, MAX_HASH,
};
pub use attempt::{SnapAttemptStore, SnapWrite};
pub use bal::{BalStateUpdate, DownloadedAccount};
pub use bootstrap::{SnapBootstrap, SnapSyncContext, SnapSyncOutcome, SnapSyncProvider};
pub use catch_up::{
    BlockAccessListCatchUp, BlockAccessListCatchUpOutcome, BlockAccessListProgress,
};
pub use context::NodeSnapContext;
pub use error::SnapSyncError;
pub use generation::{SnapDownloadProgress, SnapGeneration, SnapPhase, SnapStateStore};
pub use handoff::SnapPipelineHandoff;
pub use pivot::SnapPivotPolicy;
pub use session::{
    RangeBudget, SnapSyncSession, SnapSyncSessionState, StateDownloadOutcome, StateDownloader,
};
pub use trie::TrieGenerator;
