//! Canyon consensus rule checks.

use alloy_consensus::Header;
use alloy_trie::EMPTY_ROOT_HASH;
use reth_consensus::ConsensusError;
use reth_optimism_primitives::OpBlockBody;
use reth_primitives::GotExpected;

use crate::OpConsensusError;

/// Verifies that withdrawals in block body (Shanghai) is always empty in Canyon.
/// <https://specs.optimism.io/protocol/rollup-node-p2p.html#block-validation>
#[inline]
pub fn verify_empty_shanghai_withdrawals(body: &OpBlockBody) -> Result<(), OpConsensusError> {
    // Shanghai rule
    let withdrawals = body.withdrawals.as_ref().ok_or(ConsensusError::BodyWithdrawalsMissing)?;

    //  Canyon rule
    if !withdrawals.is_empty() {
        return Err(OpConsensusError::WithdrawalsNonEmpty)
    }

    Ok(())
}

/// Verifies that withdrawals root in block header (Shanghai) is always [`EMPTY_ROOT_HASH`] in
/// Canyon.
#[inline]
pub fn verify_empty_withdrawals_root(header: &Header) -> Result<(), ConsensusError> {
    // Shanghai rule
    let header_withdrawals_root =
        &header.withdrawals_root.ok_or(ConsensusError::WithdrawalsRootMissing)?;

    //  Canyon rules
    if *header_withdrawals_root != EMPTY_ROOT_HASH {
        return Err(ConsensusError::BodyWithdrawalsRootDiff(
            GotExpected { got: *header_withdrawals_root, expected: EMPTY_ROOT_HASH }.into(),
        ));
    }

    Ok(())
}
