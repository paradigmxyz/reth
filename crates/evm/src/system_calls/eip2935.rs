//! [EIP-2935](https://eips.ethereum.org/EIPS/eip-2935) system call implementation.

use core::fmt::Display;

use alloc::string::ToString;
use alloy_eips::eip2935::HISTORY_STORAGE_ADDRESS;

use crate::Evm;
use alloy_primitives::B256;
use reth_chainspec::EthereumHardforks;
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use revm_primitives::ResultAndState;

/// Applies the pre-block call to the [EIP-2935] blockhashes contract, using the given block,
/// chain specification, and EVM.
///
/// If Prague is not activated, or the block is the genesis block, then this is a no-op, and no
/// state changes are made.
///
/// Note: this does not commit the state changes to the database, it only transact the call.
///
/// Returns `None` if Prague is not active or the block is the genesis block, otherwise returns the
/// result of the call.
///
/// [EIP-2935]: https://eips.ethereum.org/EIPS/eip-2935
#[inline]
pub(crate) fn transact_blockhashes_contract_call(
    chain_spec: impl EthereumHardforks,
    block_timestamp: u64,
    block_number: u64,
    parent_block_hash: B256,
    evm: &mut impl Evm<Error: Display>,
) -> Result<Option<ResultAndState>, BlockExecutionError> {
    if !chain_spec.is_prague_active_at_timestamp(block_timestamp) {
        return Ok(None)
    }

    // if the block number is zero (genesis block) then no system transaction may occur as per
    // EIP-2935
    if block_number == 0 {
        return Ok(None)
    }

    let mut res = match evm.transact_system_call(
        alloy_eips::eip4788::SYSTEM_ADDRESS,
        HISTORY_STORAGE_ADDRESS,
        parent_block_hash.0.into(),
    ) {
        Ok(res) => res,
        Err(e) => {
            return Err(BlockValidationError::BlockHashContractCall { message: e.to_string() }.into())
        }
    };

    // NOTE: Revm currently marks these accounts as "touched" when we do the above transact calls,
    // and includes them in the result.
    //
    // There should be no state changes to these addresses anyways as a result of this system call,
    // so we can just remove them from the state returned.
    res.state.remove(&alloy_eips::eip4788::SYSTEM_ADDRESS);
    res.state.remove(&evm.block().coinbase);

    Ok(Some(res))
}
