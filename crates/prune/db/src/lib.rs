//! Database integration with prune implementation for MDBX.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod builder;
mod db_ext;
pub(crate) mod receipts;
mod set;
mod user;

pub use builder::{build, build_with_provider_factory};
pub use set::from_components;
pub use user::{
    AccountHistory, MerkleChangeSets, Receipts as UserReceipts, SenderRecovery, StorageHistory,
    TransactionLookup,
};
