#![doc = include_str!("../README.md")]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod account;
mod attempt;
mod bootstrap;
mod bytecode;
mod catch_up;
mod common;
mod error;
mod generation;
mod pivot;
mod reorg;
mod repair;
mod session;
mod storage;
mod verify;

#[cfg(test)]
mod test_utils;

pub use account::{
    AccountCoverage, AccountRangeDownload, AccountRangeStep, SnapAccountStore, VerifiedRange,
};
pub use attempt::{SnapAttemptStore, SnapWrite};
pub use bootstrap::{
    SnapBootstrap, SnapBootstrapOutcome, SnapSyncContext, DEFAULT_RANGES_PER_CHECK,
};
pub use bytecode::{BytecodeDownload, BytecodeStep, SnapBytecodeStore, DEFAULT_CODE_HASHES};
pub use catch_up::{
    BalStateUpdate, BlockAccessListCatchUp, CatchUpProgress, CatchUpStep, DownloadedAccount,
    SnapCatchUpStore, DEFAULT_BAL_RESPONSE_BYTES, DEFAULT_CATCH_UP_BLOCKS,
};
pub use common::{DEFAULT_RESPONSE_BYTES, MAX_HASH};
pub use error::SnapSyncError;
pub use generation::SnapGeneration;
pub use pivot::SnapPivotPolicy;
pub use reorg::SnapReorg;
pub use repair::StateRepairs;
pub use session::{SnapSyncSession, SnapSyncSessionState};
pub use storage::{
    SnapStorageStore, StorageChunk, StorageProgress, StorageRangeDownload, StorageRangeStep,
    DEFAULT_REPAIR_SLOTS, DEFAULT_STORAGE_ACCOUNTS, DEFAULT_STORAGE_REQUESTS,
};
pub use verify::{SnapStateVerifier, VerifiedSnapState, DEFAULT_SCAN_CHUNK};
