use reth_consensus::ConsensusError;

/// Scroll consensus error.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ScrollConsensusError {
    /// L1 [`ConsensusError`], that also occurs on L2.
    #[error(transparent)]
    Eth(#[from] ConsensusError),
    /// Block body has non-empty withdrawals list.
    #[error("non-empty block body withdrawals list")]
    WithdrawalsNonEmpty,
    /// Chain spec yielded unexpected blob params.
    #[error("unexpected blob params at timestamp")]
    UnexpectedBlobParams,
}
