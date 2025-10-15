//! Pruning implementation for MDBX.

pub(crate) mod receipts;

mod builder;
mod db_ext;
mod set;
mod static_file;
mod user;

pub use builder::{build, build_with_provider_factory};
pub use set::from_components;
pub use static_file::{
    Headers as StaticFileHeaders, Receipts as StaticFileReceipts,
    Transactions as StaticFileTransactions,
};
pub use user::{
    AccountHistory, MerkleChangeSets, Receipts as UserReceipts, ReceiptsByLogs, SenderRecovery,
    StorageHistory, TransactionLookup,
};
