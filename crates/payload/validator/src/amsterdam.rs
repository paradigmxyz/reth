//! Amsterdam rules for new payloads.

use alloy_rpc_types_engine::PayloadError;
use reth_primitives_traits::BlockBody;

/// Checks that block body contains withdrawals if Amsterdam is active and vv.
#[inline]
pub fn ensure_well_formed_fields<T: BlockBody>(
    block_body: &T,
    is_amsterdam_active: bool,
) -> Result<(), PayloadError> {
    if is_amsterdam_active {
        if block_body.block_access_list().is_none() {
            // amsterdam active but no block access list present
            return Err(PayloadError::PostAmsterdamBlockWithoutBlockAccessList)
        }
    } else if block_body.block_access_list().is_some() {
        // amsterdam not active but block access list present
        return Err(PayloadError::PreAmsterdamBlockWithBlockAccessList)
    }

    Ok(())
}
