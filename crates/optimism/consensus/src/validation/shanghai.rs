//! L2 Shanghai consensus rule checks.

use crate::OpConsensusError;
use reth_consensus::ConsensusError;
use reth_primitives_traits::BlockBody;

/// Verifies that withdrawals in block body (Shanghai) is always empty in Canyon.
/// <https://specs.optimism.io/protocol/rollup-node-p2p.html#block-validation>
#[inline]
pub fn ensure_empty_shanghai_withdrawals<T: BlockBody>(body: &T) -> Result<(), OpConsensusError> {
    // Shanghai rule
    let withdrawals = body.withdrawals().ok_or(ConsensusError::BodyWithdrawalsMissing)?;

    //  Canyon rule
    if !withdrawals.as_ref().is_empty() {
        return Err(OpConsensusError::WithdrawalsNonEmpty)
    }

    Ok(())
}
