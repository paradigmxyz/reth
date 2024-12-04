//! Optimism consensus errors

use derive_more::{Display, Error};

/// Optimism consensus error.
#[derive(Debug, PartialEq, Eq, Clone, Display, Error)]
pub enum OpConsensusError {
    /// Block body has non-empty withdrawals list.
    #[display("non-empty withdrawals list")]
    WithdrawalsNonEmpty,
}
