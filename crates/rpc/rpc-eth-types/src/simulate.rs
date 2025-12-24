//! Utilities for serving `eth_simulateV1`

use crate::{
    error::{api::FromEthApiError, FromEvmError, ToRpcError},
    EthApiError,
};
use alloy_consensus::{transaction::TxHashRef, BlockHeader, Transaction as _};
use alloy_eips::eip2718::WithEncoded;
use alloy_network::TransactionBuilder;
use alloy_rpc_types_eth::{
    simulate::{SimCallResult, SimulateError, SimulatedBlock},
    BlockTransactionsKind,
};
use jsonrpsee_types::ErrorObject;
use reth_evm::{
    execute::{BlockBuilder, BlockBuilderOutcome, BlockExecutor},
    Evm, HaltReasonFor,
};
use reth_primitives_traits::{BlockBody as _, BlockTy, NodePrimitives, Recovered, RecoveredBlock};
use reth_rpc_convert::{RpcBlock, RpcConvert, RpcTxReq};
use reth_rpc_server_types::result::rpc_err;
use reth_storage_api::noop::NoopProvider;
use revm::{
    context::Block,
    context_interface::result::ExecutionResult,
    primitives::{Address, Bytes, TxKind, U256},
    Database,
};

/// Errors which may occur during `eth_simulateV1` execution.
#[derive(Debug, thiserror::Error)]
pub enum EthSimulateError {
    /// Total gas limit of transactions for the block exceeds the block gas limit.
    #[error("Block gas limit exceeded by the block's transactions")]
    BlockGasLimitExceeded,
    /// Max gas limit for entire operation exceeded.
    #[error("Client adjustable limit reached")]
    GasLimitReached,
    /// Block number in sequence did not increase.
    #[error("Block number in sequence did not increase")]
    BlockNumberInvalid,
    /// Block timestamp in sequence did not increase or stay the same.
    #[error("Block timestamp in sequence did not increase")]
    BlockTimestampInvalid,
    /// Transaction nonce is too low.
    #[error("nonce too low: next nonce {state}, tx nonce {tx}")]
    NonceTooLow {
        /// Transaction nonce.
        tx: u64,
        /// Current state nonce.
        state: u64,
    },
    /// Transaction nonce is too high.
    #[error("nonce too high")]
    NonceTooHigh,
    /// Transaction's baseFeePerGas is too low.
    #[error("max fee per gas less than block base fee")]
    BaseFeePerGasTooLow,
    /// Not enough gas provided to pay for intrinsic gas.
    #[error("intrinsic gas too low")]
    IntrinsicGasTooLow,
    /// Insufficient funds to pay for gas fees and value.
    #[error("insufficient funds for gas * price + value: have {balance} want {cost}")]
    InsufficientFunds {
        /// Transaction cost.
        cost: U256,
        /// Sender balance.
        balance: U256,
    },
    /// Sender is not an EOA.
    #[error("sender is not an EOA")]
    SenderNotEOA,
    /// Max init code size exceeded.
    #[error("max initcode size exceeded")]
    MaxInitCodeSizeExceeded,
    /// `MovePrecompileToAddress` referenced itself in replacement.
    #[error("MovePrecompileToAddress referenced itself")]
    PrecompileSelfReference,
    /// Multiple `MovePrecompileToAddress` referencing the same address.
    #[error("Multiple MovePrecompileToAddress referencing the same address")]
    PrecompileDuplicateAddress,
}

impl EthSimulateError {
    /// Returns the JSON-RPC error code for a `eth_simulateV1` error.
    pub const fn error_code(&self) -> i32 {
        match self {
            Self::NonceTooLow { .. } => -38010,
            Self::NonceTooHigh => -38011,
            Self::BaseFeePerGasTooLow => -38012,
            Self::IntrinsicGasTooLow => -38013,
            Self::InsufficientFunds { .. } => -38014,
            Self::BlockGasLimitExceeded => -38015,
            Self::BlockNumberInvalid => -38020,
            Self::BlockTimestampInvalid => -38021,
            Self::PrecompileSelfReference => -38022,
            Self::PrecompileDuplicateAddress => -38023,
            Self::SenderNotEOA => -38024,
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
    converter: &T,
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
            builder.evm().block().basefee(),
            chain_id,
            builder.evm_mut().db_mut(),
            converter,
        )?;
        // Create transaction with an empty envelope.
        // The effect for a layer-2 execution client is that it does not charge L1 cost.
        let tx = WithEncoded::new(Default::default(), tx);

        builder
            .execute_transaction_with_result_closure(tx, |result| results.push(result.clone()))?;
    }

    // Pass noop provider to skip state root calculations.
    let result = builder.finish(NoopProvider::default(), false)?;

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
    converter: &T,
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

    if tx.as_ref().nonce().is_none() {
        tx.as_mut().set_nonce(
            db.basic(from).map_err(Into::into)?.map(|acc| acc.nonce).unwrap_or_default(),
        );
    }

    if tx.as_ref().gas_limit().is_none() {
        tx.as_mut().set_gas_limit(default_gas_limit);
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
            if tx.as_ref().gas_price().is_none() {
                tx.as_mut().set_gas_price(block_base_fee_per_gas as u128);
            }
        } else {
            // set dynamic 1559 fees
            if tx.as_ref().max_fee_per_gas().is_none() {
                let mut max_fee_per_gas = block_base_fee_per_gas as u128;
                if let Some(prio_fee) = tx.as_ref().max_priority_fee_per_gas() {
                    // if a prio fee is provided we need to select the max fee accordingly
                    // because the base fee must be higher than the prio fee.
                    max_fee_per_gas = prio_fee.max(max_fee_per_gas);
                }
                tx.as_mut().set_max_fee_per_gas(max_fee_per_gas);
            }
            if tx.as_ref().max_priority_fee_per_gas().is_none() {
                tx.as_mut().set_max_priority_fee_per_gas(0);
            }
        }
    }

    let tx =
        converter.build_simulate_v1_transaction(tx).map_err(|e| EthApiError::other(e.into()))?;

    Ok(Recovered::new_unchecked(tx, from))
}

/// Handles outputs of the calls execution and builds a [`SimulatedBlock`].
pub fn build_simulated_block<Err, T>(
    block: RecoveredBlock<BlockTy<T::Primitives>>,
    results: Vec<ExecutionResult<HaltReasonFor<T::Evm>>>,
    txs_kind: BlockTransactionsKind,
    converter: &T,
) -> Result<SimulatedBlock<RpcBlock<T::Network>>, Err>
where
    Err: std::error::Error
        + FromEthApiError
        + FromEvmError<T::Evm>
        + From<T::Error>
        + Into<jsonrpsee_types::ErrorObject<'static>>,
    T: RpcConvert,
{
    let mut calls: Vec<SimCallResult> = Vec::with_capacity(results.len());

    let mut log_index = 0;
    for (index, (result, tx)) in results.into_iter().zip(block.body().transactions()).enumerate() {
        let call = match result {
            ExecutionResult::Halt { reason, gas_used } => {
                let error = Err::from_evm_halt(reason, tx.gas_limit());
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
                let error = Err::from_revert(output.clone());
                SimCallResult {
                    return_data: output,
                    error: Some(SimulateError {
                        message: error.to_string(),
                        code: error.into().code(),
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
        |tx, tx_info| converter.fill(tx, tx_info),
        |header, size| converter.convert_header(header, size),
    )?;
    Ok(SimulatedBlock { inner: block, calls })
}
