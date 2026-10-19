//! Shanghai rules for new payloads.

use alloy_rpc_types_engine::PayloadError;
use reth_primitives_traits::BlockBody;

/// Checks that block body contains withdrawals if Shanghai is active and vv.
#[inline]
pub fn ensure_well_formed_fields<T: BlockBody>(
    block_body: &T,
    is_shanghai_active: bool,
) -> Result<(), PayloadError> {
    if is_shanghai_active {
        if block_body.withdrawals().is_none() {
            // shanghai active but no withdrawals present
            return Err(PayloadError::PostShanghaiBlockWithoutWithdrawals)
        }
    } else if block_body.withdrawals().is_some() {
        // shanghai not active but withdrawals present
        return Err(PayloadError::PreShanghaiBlockWithWithdrawals)
    }

    Ok(())
}
