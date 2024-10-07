//! [EIP-7002](https://eips.ethereum.org/EIPS/eip-7002) system call implementation.
use crate::ConfigureEvm;
use alloc::{boxed::Box, format, string::ToString, vec::Vec};
use alloy_eips::eip7002::{WithdrawalRequest, WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS};
use alloy_primitives::{bytes::Buf, Address, Bytes, FixedBytes};
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use reth_primitives::{Header, Request};
use revm::{interpreter::Host, Database, Evm};
use revm_primitives::{ExecutionResult, ResultAndState};

/// Applies the post-block call to the EIP-7002 withdrawal requests contract.
///
/// If Prague is not active at the given timestamp, then this is a no-op.
///
/// Note: this does not commit the state changes to the database, it only transact the call.
#[inline]
pub(crate) fn transact_withdrawal_requests_contract_call<EvmConfig, EXT, DB>(
    evm_config: &EvmConfig,
    evm: &mut Evm<'_, EXT, DB>,
) -> Result<ResultAndState, BlockExecutionError>
where
    DB: Database,
    DB::Error: core::fmt::Display,
    EvmConfig: ConfigureEvm<Header = Header>,
{
    // get previous env
    let previous_env = Box::new(evm.context.env().clone());

    // Fill transaction environment with the EIP-7002 withdrawal requests contract message data.
    //
    // This requirement for the withdrawal requests contract call defined by
    // [EIP-7002](https://eips.ethereum.org/EIPS/eip-7002) is:
    //
    // At the end of processing any execution block where `block.timestamp >= FORK_TIMESTAMP` (i.e.
    // after processing all transactions and after performing the block body withdrawal requests
    // validations), call the contract as `SYSTEM_ADDRESS`.
    evm_config.fill_tx_env_system_contract_call(
        &mut evm.context.evm.env,
        alloy_eips::eip7002::SYSTEM_ADDRESS,
        WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
        Bytes::new(),
    );

    let mut res = match evm.transact() {
        Ok(res) => res,
        Err(e) => {
            evm.context.evm.env = previous_env;
            return Err(BlockValidationError::WithdrawalRequestsContractCall {
                message: format!("execution failed: {e}"),
            }
            .into())
        }
    };

    // cleanup the state
    res.state.remove(&alloy_eips::eip7002::SYSTEM_ADDRESS);
    res.state.remove(&evm.block().coinbase);

    // re-set the previous env
    evm.context.evm.env = previous_env;

    Ok(res)
}

/// Parses the withdrawal requests from the execution output.
#[inline]
pub(crate) fn post_commit(result: ExecutionResult) -> Result<Vec<Request>, BlockExecutionError> {
    let mut data = match result {
        ExecutionResult::Success { output, .. } => Ok(output.into_data()),
        ExecutionResult::Revert { output, .. } => {
            Err(BlockValidationError::WithdrawalRequestsContractCall {
                message: format!("execution reverted: {output}"),
            })
        }
        ExecutionResult::Halt { reason, .. } => {
            Err(BlockValidationError::WithdrawalRequestsContractCall {
                message: format!("execution halted: {reason:?}"),
            })
        }
    }?;

    // Withdrawals are encoded as a series of withdrawal requests, each with the following
    // format:
    //
    // +------+--------+--------+
    // | addr | pubkey | amount |
    // +------+--------+--------+
    //    20      48        8

    const WITHDRAWAL_REQUEST_SIZE: usize = 20 + 48 + 8;
    let mut withdrawal_requests = Vec::with_capacity(data.len() / WITHDRAWAL_REQUEST_SIZE);
    while data.has_remaining() {
        if data.remaining() < WITHDRAWAL_REQUEST_SIZE {
            return Err(BlockValidationError::WithdrawalRequestsContractCall {
                message: "invalid withdrawal request length".to_string(),
            }
            .into())
        }

        let mut source_address = Address::ZERO;
        data.copy_to_slice(source_address.as_mut_slice());

        let mut validator_pubkey = FixedBytes::<48>::ZERO;
        data.copy_to_slice(validator_pubkey.as_mut_slice());

        let amount = data.get_u64();

        withdrawal_requests
            .push(WithdrawalRequest { source_address, validator_pubkey, amount }.into());
    }

    Ok(withdrawal_requests)
}
