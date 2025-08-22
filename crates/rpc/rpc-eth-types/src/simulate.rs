//! Utilities for serving `eth_simulateV1`

use crate::{
    error::{
        api::{FromEthApiError, FromEvmHalt},
        ToRpcError,
    },
    EthApiError, RevertError,
};
use alloy_consensus::{BlockHeader, Transaction as _};
use alloy_eips::eip2718::WithEncoded;
use alloy_network::TransactionBuilder;
use alloy_rpc_types_eth::{
    simulate::{SimCallResult, SimulateError, SimulatedBlock},
    BlockTransactionsKind,
};
use jsonrpsee_types::ErrorObject;
use reth_evm::{
    execute::{BlockBuilder, BlockBuilderOutcome, BlockExecutor},
    Evm,
};
use reth_primitives_traits::{
    BlockBody as _, BlockTy, NodePrimitives, Recovered, RecoveredBlock, SignedTransaction,
};
use reth_rpc_convert::{RpcBlock, RpcConvert, RpcTxReq};
use reth_rpc_server_types::result::rpc_err;
use reth_storage_api::noop::NoopProvider;
use revm::{
    context_interface::result::ExecutionResult,
    primitives::{Address, Bytes, TxKind, U256},
    Database,
};

/// Errors which may occur during `eth_simulateV1` execution.
///
/// These error codes are defined in the execution-apis specification: <https://github.com/ethereum/execution-apis/blob/e92738bba4dee798c555dbc371c36c4aaff67779/src/eth/execute.yaml#L98>
#[derive(Debug, thiserror::Error)]
pub enum EthSimulateError {
    /// Transaction nonce is too low.
    #[error("Transactions nonce is too low")]
    NonceTooLow,
    /// Transaction nonce is too high.
    #[error("Transactions nonce is too high")]
    NonceTooHigh,
    /// Transaction base fee per gas is too low.
    #[error("Transactions baseFeePerGas is too low")]
    BaseFeePerGasTooLow,
    /// Not enough gas provided to pay for intrinsic gas for a transaction.
    #[error("Not enough gas provided to pay for intrinsic gas for a transaction")]
    IntrinsicGasTooLow,
    /// Insufficient funds to pay for gas fees and value for a transaction.
    #[error("Insufficient funds to pay for gas fees and value for a transaction")]
    InsufficientFunds,
    /// Total gas limit of transactions for the block exceeds the block gas limit.
    #[error("Block gas limit exceeded by the block's transactions")]
    BlockGasLimitExceeded,
    /// Block number in sequence did not increase.
    #[error("Block number in sequence did not increase")]
    BlockNumberNotIncreased,
    /// Block timestamp in sequence did not increase or stay the same.
    #[error("Block timestamp in sequence did not increase or stay the same")]
    BlockTimestampNotIncreased,
    /// `MovePrecompileToAddress` referenced itself in replacement.
    #[error("MovePrecompileToAddress referenced itself in replacement")]
    PrecompileSelfReference,
    /// Multiple `MovePrecompileToAddress` referencing the same address to replace.
    #[error("Multiple MovePrecompileToAddress referencing the same address to replace")]
    PrecompileMultipleReference,
    /// Sender is not an EOA.
    #[error("Sender is not an EOA")]
    SenderNotEoa,
    /// Max init code size exceeded.
    #[error("Max init code size exceeded")]
    MaxInitCodeSizeExceeded,
    /// Max gas limit for entire operation exceeded.
    #[error("Client adjustable limit reached")]
    GasLimitReached,
}

impl EthSimulateError {
    const fn error_code(&self) -> i32 {
        match self {
            Self::NonceTooLow => -38010,
            Self::NonceTooHigh => -38011,
            Self::BaseFeePerGasTooLow => -38012,
            Self::IntrinsicGasTooLow => -38013,
            Self::InsufficientFunds => -38014,
            Self::BlockGasLimitExceeded => -38015,
            Self::BlockNumberNotIncreased => -38020,
            Self::BlockTimestampNotIncreased => -38021,
            Self::PrecompileSelfReference => -38022,
            Self::PrecompileMultipleReference => -38023,
            Self::SenderNotEoa => -38024,
            Self::MaxInitCodeSizeExceeded => -38025,
            Self::GasLimitReached => -38026,
        }
    }
}

impl ToRpcError for EthSimulateError {
    fn to_rpc_error(&self) -> ErrorObject<'static> {
        rpc_err(self.error_code(), self.to_string(), None)
    }
}

impl From<EthSimulateError> for EthApiError {
    fn from(err: EthSimulateError) -> Self {
        Self::other(err)
    }
}

/// Converts all [`TransactionRequest`]s into [`Recovered`] transactions and applies them to the
/// given [`BlockExecutor`].
///
/// Returns all executed transactions and the result of the execution.
///
/// [`TransactionRequest`]: alloy_rpc_types_eth::TransactionRequest
#[expect(clippy::type_complexity)]
pub fn execute_transactions<S, T>(
    mut builder: S,
    calls: Vec<RpcTxReq<T::Network>>,
    default_gas_limit: u64,
    chain_id: u64,
    tx_resp_builder: &T,
) -> Result<
    (
        BlockBuilderOutcome<S::Primitives>,
        Vec<ExecutionResult<<<S::Executor as BlockExecutor>::Evm as Evm>::HaltReason>>,
    ),
    EthApiError,
>
where
    S: BlockBuilder<Executor: BlockExecutor<Evm: Evm<DB: Database<Error: Into<EthApiError>>>>>,
    T: RpcConvert<Primitives = S::Primitives>,
{
    builder.apply_pre_execution_changes()?;

    let mut results = Vec::with_capacity(calls.len());
    for call in calls {
        // Resolve transaction, populate missing fields and enforce calls
        // correctness.
        let tx = resolve_transaction(
            call,
            default_gas_limit,
            builder.evm().block().basefee,
            chain_id,
            builder.evm_mut().db_mut(),
            tx_resp_builder,
        )?;
        // Create transaction with an empty envelope.
        // The effect for a layer-2 execution client is that it does not charge L1 cost.
        let tx = WithEncoded::new(Default::default(), tx);

        builder
            .execute_transaction_with_result_closure(tx, |result| results.push(result.clone()))?;
    }

    // Pass noop provider to skip state root calculations.
    let result = builder.finish(NoopProvider::default())?;

    Ok((result, results))
}

/// Goes over the list of [`TransactionRequest`]s and populates missing fields trying to resolve
/// them into primitive transactions.
///
/// This will set the defaults as defined in <https://github.com/ethereum/execution-apis/blob/e56d3208789259d0b09fa68e9d8594aa4d73c725/docs/ethsimulatev1-notes.md#default-values-for-transactions>
///
/// [`TransactionRequest`]: alloy_rpc_types_eth::TransactionRequest
pub fn resolve_transaction<DB: Database, Tx, T>(
    mut tx: RpcTxReq<T::Network>,
    default_gas_limit: u64,
    block_base_fee_per_gas: u64,
    chain_id: u64,
    db: &mut DB,
    tx_resp_builder: &T,
) -> Result<Recovered<Tx>, EthApiError>
where
    DB::Error: Into<EthApiError>,
    T: RpcConvert<Primitives: NodePrimitives<SignedTx = Tx>>,
{
    // If we're missing any fields we try to fill nonce, gas and
    // gas price.
    let tx_type = tx.as_ref().output_tx_type();

    let from = if let Some(from) = tx.as_ref().from() {
        from
    } else {
        tx.as_mut().set_from(Address::ZERO);
        Address::ZERO
    };

    // Get current account state for validation
    let account = db.basic(from).map_err(Into::into)?;
    let (nonce, balance) = account.as_ref().map(|acc| (acc.nonce, acc.balance)).unwrap_or_default();

    if let Some(tx_nonce) = tx.as_ref().nonce() {
        if tx_nonce < nonce {
            return Err(EthSimulateError::NonceTooLow.into());
        }
    } else {
        // Note: We allow high nonces for simulation purposes as future transactions may be included
        tx.as_mut().set_nonce(nonce);
    }

    let gas_limit = tx.as_ref().gas_limit().unwrap_or(default_gas_limit);
    if tx.as_ref().gas_limit().is_none() {
        tx.as_mut().set_gas_limit(gas_limit);
    }

    if tx.as_ref().chain_id().is_none() {
        tx.as_mut().set_chain_id(chain_id);
    }

    if tx.as_ref().kind().is_none() {
        tx.as_mut().set_kind(TxKind::Create);
    }

    // if we can't build the _entire_ transaction yet, we need to check the fee values
    if tx.as_ref().output_tx_type_checked().is_none() {
        if tx_type.is_legacy() || tx_type.is_eip2930() {
            let gas_price = if let Some(gas_price) = tx.as_ref().gas_price() {
                if gas_price < block_base_fee_per_gas as u128 {
                    return Err(EthSimulateError::BaseFeePerGasTooLow.into());
                }
                gas_price
            } else {
                let gas_price = block_base_fee_per_gas as u128;
                tx.as_mut().set_gas_price(gas_price);
                gas_price
            };

            let value = tx.as_ref().value().unwrap_or_default();
            let max_cost = gas_price.saturating_mul(gas_limit as u128).saturating_add(value.to());
            if balance < U256::from(max_cost) {
                return Err(EthSimulateError::InsufficientFunds.into());
            }
        } else {
            // Handle EIP-1559 transactions
            let max_fee_per_gas = if let Some(max_fee) = tx.as_ref().max_fee_per_gas() {
                if max_fee < block_base_fee_per_gas as u128 {
                    return Err(EthSimulateError::BaseFeePerGasTooLow.into());
                }
                max_fee
            } else {
                let mut max_fee_per_gas = block_base_fee_per_gas as u128;
                if let Some(prio_fee) = tx.as_ref().max_priority_fee_per_gas() {
                    max_fee_per_gas = prio_fee.max(max_fee_per_gas);
                }
                tx.as_mut().set_max_fee_per_gas(max_fee_per_gas);
                max_fee_per_gas
            };

            if tx.as_ref().max_priority_fee_per_gas().is_none() {
                tx.as_mut().set_max_priority_fee_per_gas(0);
            }

            let value = tx.as_ref().value().unwrap_or_default();
            let max_cost =
                max_fee_per_gas.saturating_mul(gas_limit as u128).saturating_add(value.to());
            if balance < U256::from(max_cost) {
                return Err(EthSimulateError::InsufficientFunds.into());
            }
        }
    }

    let tx = tx_resp_builder
        .build_simulate_v1_transaction(tx)
        .map_err(|e| EthApiError::other(e.into()))?;

    Ok(Recovered::new_unchecked(tx, from))
}

/// Handles outputs of the calls execution and builds a [`SimulatedBlock`].
pub fn build_simulated_block<T, Halt: Clone>(
    block: RecoveredBlock<BlockTy<T::Primitives>>,
    results: Vec<ExecutionResult<Halt>>,
    txs_kind: BlockTransactionsKind,
    tx_resp_builder: &T,
) -> Result<SimulatedBlock<RpcBlock<T::Network>>, T::Error>
where
    T: RpcConvert<Error: FromEthApiError + FromEvmHalt<Halt>>,
{
    let mut calls: Vec<SimCallResult> = Vec::with_capacity(results.len());

    let mut log_index = 0;
    for (index, (result, tx)) in results.into_iter().zip(block.body().transactions()).enumerate() {
        let call = match result {
            ExecutionResult::Halt { reason, gas_used } => {
                let error = T::Error::from_evm_halt(reason, tx.gas_limit());
                SimCallResult {
                    return_data: Bytes::new(),
                    error: Some(SimulateError {
                        message: error.to_string(),
                        code: error.into().code(),
                    }),
                    gas_used,
                    logs: Vec::new(),
                    status: false,
                }
            }
            ExecutionResult::Revert { output, gas_used } => {
                let error = RevertError::new(output.clone());
                SimCallResult {
                    return_data: output,
                    error: Some(SimulateError {
                        code: error.error_code(),
                        message: error.to_string(),
                    }),
                    gas_used,
                    status: false,
                    logs: Vec::new(),
                }
            }
            ExecutionResult::Success { output, gas_used, logs, .. } => SimCallResult {
                return_data: output.into_data(),
                error: None,
                gas_used,
                logs: logs
                    .into_iter()
                    .map(|log| {
                        log_index += 1;
                        alloy_rpc_types_eth::Log {
                            inner: log,
                            log_index: Some(log_index - 1),
                            transaction_index: Some(index as u64),
                            transaction_hash: Some(*tx.tx_hash()),
                            block_number: Some(block.header().number()),
                            block_timestamp: Some(block.header().timestamp()),
                            ..Default::default()
                        }
                    })
                    .collect(),
                status: true,
            },
        };

        calls.push(call);
    }

    let block = block.into_rpc_block(
        txs_kind,
        |tx, tx_info| tx_resp_builder.fill(tx, tx_info),
        |header, size| tx_resp_builder.convert_header(header, size),
    )?;
    Ok(SimulatedBlock { inner: block, calls })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eth_simulate_error_conversion() {
        // Test that EthSimulateError can be converted to EthApiError
        let simulate_error = EthSimulateError::NonceTooLow;
        let api_error: EthApiError = simulate_error.into();

        // Should be wrapped as an "other" error
        assert!(matches!(api_error, EthApiError::Other(_)));
    }
}
