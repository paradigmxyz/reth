//! Canyon consensus rule checks.

use alloy_consensus::BlockHeader;
use alloy_trie::EMPTY_ROOT_HASH;
use reth_consensus::ConsensusError;
use reth_primitives_traits::GotExpected;

/// Verifies that withdrawals root in block header (Shanghai) is always [`EMPTY_ROOT_HASH`] in
/// Canyon.
#[inline]
pub fn ensure_empty_withdrawals_root<H: BlockHeader>(header: &H) -> Result<(), ConsensusError> {
    // Shanghai rule
    let header_withdrawals_root =
        &header.withdrawals_root().ok_or(ConsensusError::WithdrawalsRootMissing)?;

    //  Canyon rules
    if *header_withdrawals_root != EMPTY_ROOT_HASH {
        return Err(ConsensusError::BodyWithdrawalsRootDiff(
            GotExpected { got: *header_withdrawals_root, expected: EMPTY_ROOT_HASH }.into(),
        ));
    }

    Ok(())
}
