//! Account-range download coordination and persistence.

mod download;
mod store;

pub use download::{AccountRangeDownload, AccountRangeStep, VerifiedRange};
pub(crate) use store::StoredCoverage;
pub use store::{AccountCoverage, SnapAccountStore};
