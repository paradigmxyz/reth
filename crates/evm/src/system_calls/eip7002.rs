//! [EIP-7002](https://eips.ethereum.org/EIPS/eip-7002) system call implementation.
use crate::Evm;
use alloc::format;
use alloy_eips::eip7002::WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS;
use alloy_primitives::Bytes;
use core::fmt::Display;
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use revm_primitives::{ExecutionResult, ResultAndState};

/// Applies the post-block call to the EIP-7002 withdrawal requests contract.
///
/// If Prague is not active at the given timestamp, then this is a no-op.
///
/// Note: this does not commit the state changes to the database, it only transact the call.
#[inline]
pub(crate) fn transact_withdrawal_requests_contract_call(
    evm: &mut impl Evm<Error: Display>,
) -> Result<ResultAndState, BlockExecutionError> {
    // Execute EIP-7002 withdrawal requests contract message data.
    //
    // This requirement for the withdrawal requests contract call defined by
    // [EIP-7002](https://eips.ethereum.org/EIPS/eip-7002) is:
    //
    // At the end of processing any execution block where `block.timestamp >= FORK_TIMESTAMP` (i.e.
    // after processing all transactions and after performing the block body withdrawal requests
    // validations), call the contract as `SYSTEM_ADDRESS`.
    let mut res = match evm.transact_system_call(
        alloy_eips::eip7002::SYSTEM_ADDRESS,
        WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
        Bytes::new(),
    ) {
        Ok(res) => res,
        Err(e) => {
            return Err(BlockValidationError::WithdrawalRequestsContractCall {
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

/// Calls the withdrawals requests system contract, and returns the requests from the execution
/// output.
#[inline]
pub(crate) fn post_commit(result: ExecutionResult) -> Result<Bytes, BlockExecutionError> {
    match result {
        ExecutionResult::Success { output, .. } => Ok(output.into_data()),
        ExecutionResult::Revert { output, .. } => {
            Err(BlockValidationError::WithdrawalRequestsContractCall {
                message: format!("execution reverted: {output}"),
            }
            .into())
        }
        ExecutionResult::Halt { reason, .. } => {
            Err(BlockValidationError::WithdrawalRequestsContractCall {
                message: format!("execution halted: {reason:?}"),
            }
            .into())
        }
    }
}
