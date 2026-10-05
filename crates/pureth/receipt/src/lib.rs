#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

pub mod eip6466;
mod provider;
mod tree;

pub use eip6466::{
    receipts_from_block, AuthorizationOutcome, BasicReceipt, BlockAuthorizationOutcomes,
    CreateReceipt, Eip6466ReceiptSnapshot, Eip6466SnapshotError, Log, Receipt,
    ReceiptConstructionError, ReceiptSerializationError, Receipts, SetCodeReceipt,
    TransactionAuthorizationOutcomes,
};
pub use provider::{
    CanonicalityStatus, DeterministicProvider, HistoricalAcquisitionError, LookupError, ObjectKind,
    ProviderBuildError, ProviderSnapshot, RethRootProvider, RethRootProviderError, RootContext,
    MULTIPLE_LOGS_BLOCK_HASH, PROGRESSIVE_RECEIPTS_BLOCK_HASH, SINGLETON_BLOCK_HASH,
};
pub use tree::{RetainedNode, TreeConstructionError};

#[cfg(any(test, feature = "test-utils"))]
pub use provider::test_utils;
