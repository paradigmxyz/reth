//! Optimism consensus errors

use alloy_primitives::B256;
use derive_more::{Display, Error, From};
use reth_consensus::ConsensusError;
use reth_storage_errors::provider::ProviderError;

/// Optimism consensus error.
#[derive(Debug, PartialEq, Eq, Clone, Display, Error, From)]
pub enum OpConsensusError {
    /// Block body has non-empty withdrawals list (l1 withdrawals).
    #[display("non-empty block body withdrawals list")]
    WithdrawalsNonEmpty,
    /// Failed to compute storage root of
    /// [l2tol1-message-passer](reth_optimism_primitives::ADDRESS_L2_TO_L1_MESSAGE_PASSER).
    #[display("compute withdrawals storage root failed: {_0}")]
    #[from]
    StorageRootCalculationFail(ProviderError),
    /// Storage root of
    /// [l2tol1-message-passer](reth_optimism_primitives::ADDRESS_L2_TO_L1_MESSAGE_PASSER) missing
    /// in block header (withdrawals root field).
    #[display("withdrawals storage root missing from block header")]
    StorageRootMissing,
    /// Storage root of
    /// [l2tol1-message-passer](reth_optimism_primitives::ADDRESS_L2_TO_L1_MESSAGE_PASSER)
    /// in block header (withdrawals field), doesn't match local storage root.
    #[display("withdrawals storage root mismatch, header: {header}, exec_res: {exec_res}")]
    StorageRootMismatch {
        /// Storage root of pre-deploy in block.
        header: B256,
        /// Storage root of pre-deploy loaded from local state.
        exec_res: B256,
    },
    /// L1 [`ConsensusError`], that also occurs on L2.
    #[display("{_0}")]
    #[from]
    Eth(ConsensusError),
}
