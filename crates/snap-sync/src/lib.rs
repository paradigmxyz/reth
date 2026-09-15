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

mod attempt;
mod bootstrap;
mod catch_up;
mod common;
mod context;
mod error;
mod generation;
mod handoff;
mod pivot;
mod session;
mod state;
mod trie;

#[cfg(test)]
mod test_utils;

pub use attempt::{SnapAttemptStore, SnapWrite};
pub use bootstrap::{SnapBootstrap, SnapSyncOutcome};
pub use catch_up::{
    BalStateUpdate, BlockAccessListCatchUp, BlockAccessListCatchUpOutcome, BlockAccessListProgress,
    DownloadedAccount,
};
pub use common::{DEFAULT_RESPONSE_BYTES, MAX_HASH};
pub use context::{NodeSnapContext, SnapSyncContext, SnapSyncProvider};
pub use error::SnapSyncError;
pub use generation::{SnapDownloadProgress, SnapGeneration, SnapPhase, SnapStateStore};
pub use handoff::SnapPipelineHandoff;
pub use pivot::SnapPivotPolicy;
pub use session::{SnapSyncSession, SnapSyncSessionState};
pub use state::{
    AccountCoverage, AccountRangeDownload, AccountRangeProgress, AccountRangeStep, RangeBudget,
    SnapAccountStore, StateDownloadOutcome, StateDownloader, VerifiedRange,
};
pub use trie::TrieGenerator;
