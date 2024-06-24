//! Bundle state module.
//! This module contains all the logic related to bundle state.
#[cfg(feature = "not-zkvm")]
mod bundle_state_with_receipts;
mod hashed_state_changes;
mod state_changes;
mod state_reverts;

#[cfg(feature = "not-zkvm")]
pub use bundle_state_with_receipts::{
    AccountRevertInit, BundleStateInit, ExecutionOutcome, OriginalValuesKnown, RevertsInit,
};
pub use hashed_state_changes::HashedStateChanges;
pub use state_changes::StateChanges;
pub use state_reverts::{StateReverts, StorageRevertsIter};
