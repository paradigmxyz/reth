//! [EIP-7251](https://eips.ethereum.org/EIPS/eip-7251) system call implementation.
use crate::Evm;
use alloc::format;
use alloy_eips::eip7251::CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS;
use alloy_primitives::Bytes;
use core::fmt::Display;
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use revm_primitives::{ExecutionResult, ResultAndState};

/// Applies the post-block call to the EIP-7251 consolidation requests contract.
///
/// If Prague is not active at the given timestamp, then this is a no-op, and an empty vector is
/// returned. Otherwise, the consolidation requests are returned.
///
/// Note: this does not commit the state changes to the database, it only transact the call.
#[inline]
pub(crate) fn transact_consolidation_requests_contract_call(
    evm: &mut impl Evm<Error: Display>,
) -> Result<ResultAndState, BlockExecutionError> {
    // Execute EIP-7251 consolidation requests contract message data.
    //
    // This requirement for the consolidation requests contract call defined by
    // [EIP-7251](https://eips.ethereum.org/EIPS/eip-7251) is:
    //
    // At the end of processing any execution block where block.timestamp >= FORK_TIMESTAMP (i.e.
    // after processing all transactions and after performing the block body requests validations)
    // clienst software MUST [..] call the contract as `SYSTEM_ADDRESS` and empty input data to
    // trigger the system subroutine execute.
    let mut res = match evm.transact_system_call(
        alloy_eips::eip7002::SYSTEM_ADDRESS,
        CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
        Bytes::new(),
    ) {
        Ok(res) => res,
        Err(e) => {
            return Err(BlockValidationError::ConsolidationRequestsContractCall {
                message: format!("execution failed: {e}"),
            }
            .into())
        }
    };

    // NOTE: Revm currently marks these accounts as "touched" when we do the above transact calls,
    // and includes them in the result.
    //
    // There should be no state changes to these addresses anyways as a result of this system call,
    // so we can just remove them from the state returned.
    res.state.remove(&alloy_eips::eip7002::SYSTEM_ADDRESS);
    res.state.remove(&evm.block().coinbase);

    Ok(res)
}

/// Calls the consolidation requests system contract, and returns the requests from the execution
/// output.
#[inline]
pub(crate) fn post_commit(result: ExecutionResult) -> Result<Bytes, BlockExecutionError> {
    match result {
        ExecutionResult::Success { output, .. } => Ok(output.into_data()),
        ExecutionResult::Revert { output, .. } => {
            Err(BlockValidationError::ConsolidationRequestsContractCall {
                message: format!("execution reverted: {output}"),
            }
            .into())
        }
        ExecutionResult::Halt { reason, .. } => {
            Err(BlockValidationError::ConsolidationRequestsContractCall {
                message: format!("execution halted: {reason:?}"),
            }
            .into())
        }
    }
}
