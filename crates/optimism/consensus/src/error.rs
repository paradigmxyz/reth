//! Optimism consensus errors

use alloy_primitives::B256;
use derive_more::{Display, Error, From};
use reth_storage_errors::ProviderError;

/// Optimism consensus error.
#[derive(Debug, PartialEq, Eq, Clone, Display, Error, From)]
pub enum OpConsensusError {
    /// Block body has non-empty withdrawals list.
    #[display("non-empty withdrawals list")]
    WithdrawalsNonEmpty,
    /// Failed to load storage root of
    /// [`L2toL1MessagePasser`](reth_optimism_primitives::ADDRESS_L2_TO_L1_MESSAGE_PASSER).
    #[display("failed to load storage root of L2toL1MessagePasser pre-deploy: {_0}")]
    #[from]
    LoadStorageRootFailed(ProviderError),
    /// Storage root of
    /// [`L2toL1MessagePasser`](reth_optimism_primitives::ADDRESS_L2_TO_L1_MESSAGE_PASSER) missing
    /// in block (withdrawals root field).
    #[display("storage root of L2toL1MessagePasser missing (withdrawals root field empty)")]
    StorageRootMissing,
    /// Storage root of
    /// [`L2toL1MessagePasser`](reth_optimism_primitives::ADDRESS_L2_TO_L1_MESSAGE_PASSER)
    /// in block (withdrawals field), doesn't match local storage root.
    #[display("L2toL1MessagePasser storage root mismatch, got: {}, expected {expected}", got.map(|hash| hash.to_string()).unwrap_or_else(|| "null".to_string()))]
    StorageRootMismatch {
        /// Storage root of pre-deploy in block.
        got: Option<B256>,
        /// Storage root of pre-deploy loaded from local state.
        expected: B256,
    },
}
