//! [EIP-7251](https://eips.ethereum.org/EIPS/eip-7251) system call implementation.
use crate::ConfigureEvm;
use alloc::{boxed::Box, format, string::ToString, vec::Vec};
use alloy_eips::eip7251::{ConsolidationRequest, CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS};
use alloy_primitives::{bytes::Buf, Address, Bytes, FixedBytes};
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use reth_primitives::{Header, Request};
use revm::{interpreter::Host, Database, Evm};
use revm_primitives::{ExecutionResult, ResultAndState};

/// Applies the post-block call to the EIP-7251 consolidation requests contract.
///
/// If Prague is not active at the given timestamp, then this is a no-op, and an empty vector is
/// returned. Otherwise, the consolidation requests are returned.
///
/// Note: this does not commit the state changes to the database, it only transact the call.
#[inline]
pub(crate) fn transact_consolidation_requests_contract_call<EvmConfig, EXT, DB>(
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

    // Fill transaction environment with the EIP-7251 consolidation requests contract message data.
    //
    // This requirement for the consolidation requests contract call defined by
    // [EIP-7251](https://eips.ethereum.org/EIPS/eip-7251) is:
    //
    // At the end of processing any execution block where block.timestamp >= FORK_TIMESTAMP (i.e.
    // after processing all transactions and after performing the block body requests validations)
    // clienst software MUST [..] call the contract as `SYSTEM_ADDRESS` and empty input data to
    // trigger the system subroutine execute.
    evm_config.fill_tx_env_system_contract_call(
        &mut evm.context.evm.env,
        alloy_eips::eip7002::SYSTEM_ADDRESS,
        CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
        Bytes::new(),
    );

    let mut res = match evm.transact() {
        Ok(res) => res,
        Err(e) => {
            evm.context.evm.env = previous_env;
            return Err(BlockValidationError::ConsolidationRequestsContractCall {
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

/// Parses the consolidation requests from the execution output.
#[inline]
pub(crate) fn post_commit(result: ExecutionResult) -> Result<Vec<Request>, BlockExecutionError> {
    let mut data = match result {
        ExecutionResult::Success { output, .. } => Ok(output.into_data()),
        ExecutionResult::Revert { output, .. } => {
            Err(BlockValidationError::ConsolidationRequestsContractCall {
                message: format!("execution reverted: {output}"),
            })
        }
        ExecutionResult::Halt { reason, .. } => {
            Err(BlockValidationError::ConsolidationRequestsContractCall {
                message: format!("execution halted: {reason:?}"),
            })
        }
    }?;

    // Consolidations are encoded as a series of consolidation requests, each with the following
    // format:
    //
    // +------+--------+---------------+
    // | addr | pubkey | target pubkey |
    // +------+--------+---------------+
    //    20      48        48

    const CONSOLIDATION_REQUEST_SIZE: usize = 20 + 48 + 48;
    let mut consolidation_requests = Vec::with_capacity(data.len() / CONSOLIDATION_REQUEST_SIZE);
    while data.has_remaining() {
        if data.remaining() < CONSOLIDATION_REQUEST_SIZE {
            return Err(BlockValidationError::ConsolidationRequestsContractCall {
                message: "invalid consolidation request length".to_string(),
            }
            .into())
        }

        let mut source_address = Address::ZERO;
        data.copy_to_slice(source_address.as_mut_slice());

        let mut source_pubkey = FixedBytes::<48>::ZERO;
        data.copy_to_slice(source_pubkey.as_mut_slice());

        let mut target_pubkey = FixedBytes::<48>::ZERO;
        data.copy_to_slice(target_pubkey.as_mut_slice());

        consolidation_requests.push(Request::ConsolidationRequest(ConsolidationRequest {
            source_address,
            source_pubkey,
            target_pubkey,
        }));
    }

    Ok(consolidation_requests)
}
