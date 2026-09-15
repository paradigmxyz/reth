//! Account-range download coordination and persistence.

mod download;
mod store;

pub use download::{AccountRangeDownload, AccountRangeStep, VerifiedRange};
pub use store::{AccountCoverage, AccountRangeProgress, SnapAccountStore};

pub(crate) use download::account_progress;
#[cfg(test)]
pub(crate) use download::next_hash;
