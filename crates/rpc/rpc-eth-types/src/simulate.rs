//! Utilities for serving `eth_simulateV1`

use alloy_consensus::{TxEip4844Variant, TxType, TypedTransaction};
use alloy_sol_types::SolValue;
use jsonrpsee_types::ErrorObject;
use reth_primitives::{
    BlockWithSenders, Signature, Transaction, TransactionSigned, TransactionSignedNoHash,
};
use reth_rpc_server_types::result::rpc_err;
use reth_rpc_types::{
    simulate::{SimCallResult, SimulateError, SimulatedBlock},
    Block, BlockTransactionsKind, ToRpcError, TransactionRequest, WithOtherFields,
};
use reth_rpc_types_compat::block::from_block;
use revm::{interpreter::CallValue, Database, Inspector};
use revm_primitives::{
    address, b256, Address, BlockEnv, Bytes, ExecutionResult, Log, LogData, TxKind, B256, U256,
};

use crate::{EthApiError, RevertError, RpcInvalidTransactionError};

/// Sender of ETH transfer log per `eth_simulateV1` spec.
///
/// <https://github.com/ethereum/execution-apis/pull/484>
pub const TRANSFER_LOG_EMITTER: Address = address!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

/// Topic of `Transfer(address,address,uint256)` event.
pub const TRANSFER_EVENT_TOPIC: B256 =
    b256!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");

/// Helper inspector for `eth_simulateV1` which injects `Transfer` logs for all ETH transfers.
#[derive(Debug, Default, Clone)]
pub struct TransferInspector;

impl<DB: Database> Inspector<DB> for TransferInspector {
    fn call(
        &mut self,
        context: &mut revm::EvmContext<DB>,
        inputs: &mut revm::interpreter::CallInputs,
    ) -> Option<revm::interpreter::CallOutcome> {
        if let CallValue::Transfer(value) = inputs.value {
            if !value.is_zero() {
                let from = B256::from_slice(&inputs.caller.abi_encode());
                let to = B256::from_slice(&inputs.target_address.abi_encode());
                let data = value.abi_encode();

                context.journaled_state.log(Log {
                    address: TRANSFER_LOG_EMITTER,
                    data: LogData::new_unchecked(vec![TRANSFER_EVENT_TOPIC, from, to], data.into()),
                });
            }
        }

        None
    }
}

/// Errors which may occur during `eth_simulateV1` execution.
#[derive(Debug, thiserror::Error)]
pub enum EthSimulateError {
    /// Total gas limit of transactions for the block exceeds the block gas limit.
    #[error("Block gas limit exceeded by the block's transactions")]
    BlockGasLimitExceeded,
    /// Max gas limit for entire operation exceeded.
    #[error("Client adjustable limit reached")]
    GasLimitReached,
}

impl EthSimulateError {
    const fn error_code(&self) -> i32 {
        match self {
            Self::BlockGasLimitExceeded => -38015,
            Self::GasLimitReached => -38026,
        }
    }
}

impl ToRpcError for EthSimulateError {
    fn to_rpc_error(&self) -> ErrorObject<'static> {
        rpc_err(self.error_code(), self.to_string(), None)
    }
}

/// Goes over the list of [`TransactionRequest`]s and populates missing fields trying to resolve
/// them into [`TransactionSigned`].
///
/// If validation is enabled, the function will return error if any of the transactions can't be
/// built right away.
pub fn resolve_transactions<DB: Database>(
    txs: &mut [TransactionRequest],
    validation: bool,
    block_gas_limit: u128,
    chain_id: u64,
    db: &mut DB,
) -> Result<Vec<TransactionSigned>, EthApiError>
where
    EthApiError: From<DB::Error>,
{
    let mut transactions = Vec::with_capacity(txs.len());

    let default_gas_limit = {
        let total_specified_gas = txs.iter().filter_map(|tx| tx.gas).sum::<u128>();
        let txs_without_gas_limit = txs.iter().filter(|tx| tx.gas.is_none()).count();

        if total_specified_gas > block_gas_limit {
            return Err(EthApiError::Other(Box::new(EthSimulateError::BlockGasLimitExceeded)))
        }

        (block_gas_limit - total_specified_gas) / txs_without_gas_limit as u128
    };

    for tx in txs {
        if tx.buildable_type().is_none() && validation {
            return Err(EthApiError::TransactionConversionError);
        }
        // If we're missing any fields and validation is disabled, we try filling nonce, gas and
        // gas price.
        let tx_type = tx.preferred_type();

        let from = if let Some(from) = tx.from {
            from
        } else {
            tx.from = Some(Address::ZERO);
            Address::ZERO
        };

        if tx.nonce.is_none() {
            tx.nonce = Some(db.basic(from)?.map(|acc| acc.nonce).unwrap_or_default());
        }

        if tx.gas.is_none() {
            tx.gas = Some(default_gas_limit);
        }

        if tx.chain_id.is_none() {
            tx.chain_id = Some(chain_id);
        }

        if tx.to.is_none() {
            tx.to = Some(TxKind::Create);
        }

        match tx_type {
            TxType::Legacy | TxType::Eip2930 => {
                if tx.gas_price.is_none() {
                    tx.gas_price = Some(0);
                }
            }
            _ => {
                if tx.max_fee_per_gas.is_none() {
                    tx.max_fee_per_gas = Some(0);
                    tx.max_priority_fee_per_gas = Some(0);
                }
            }
        }

        let Ok(tx) = tx.clone().build_typed_tx() else {
            return Err(EthApiError::TransactionConversionError)
        };

        // Create an empty signature for the transaction.
        let signature =
            Signature { odd_y_parity: false, r: Default::default(), s: Default::default() };

        let tx = match tx {
            TypedTransaction::Legacy(tx) => {
                TransactionSignedNoHash { transaction: Transaction::Legacy(tx), signature }
                    .with_hash()
            }
            TypedTransaction::Eip2930(tx) => {
                TransactionSignedNoHash { transaction: Transaction::Eip2930(tx), signature }
                    .with_hash()
            }
            TypedTransaction::Eip1559(tx) => {
                TransactionSignedNoHash { transaction: Transaction::Eip1559(tx), signature }
                    .with_hash()
            }
            TypedTransaction::Eip4844(tx) => {
                let tx = match tx {
                    TxEip4844Variant::TxEip4844(tx) => tx,
                    TxEip4844Variant::TxEip4844WithSidecar(tx) => tx.tx,
                };
                TransactionSignedNoHash { transaction: Transaction::Eip4844(tx), signature }
                    .with_hash()
            }
            TypedTransaction::Eip7702(tx) => {
                TransactionSignedNoHash { transaction: Transaction::Eip7702(tx), signature }
                    .with_hash()
            }
        };

        transactions.push(tx);
    }

    Ok(transactions)
}

/// Handles outputs of the calls execution and builds a [`SimulatedBlock`].
pub fn build_block(
    results: Vec<(Address, ExecutionResult)>,
    transactions: Vec<TransactionSigned>,
    block_env: &BlockEnv,
    parent_hash: B256,
    total_difficulty: U256,
    full_transactions: bool,
) -> Result<SimulatedBlock<Block<WithOtherFields<reth_rpc_types::Transaction>>>, EthApiError> {
    let mut calls = Vec::with_capacity(results.len());
    let mut senders = Vec::with_capacity(results.len());

    let mut log_index = 0;
    for (transaction_index, ((sender, result), tx)) in
        results.into_iter().zip(transactions.iter()).enumerate()
    {
        senders.push(sender);

        let call = match result {
            ExecutionResult::Halt { reason, gas_used } => {
                let error = RpcInvalidTransactionError::halt(reason, tx.gas_limit());
                SimCallResult {
                    return_value: Bytes::new(),
                    error: Some(SimulateError {
                        code: error.error_code(),
                        message: error.to_string(),
                    }),
                    gas_used,
                    logs: Vec::new(),
                    status: false,
                }
            }
            ExecutionResult::Revert { output, gas_used } => {
                let error = RevertError::new(output.clone());
                SimCallResult {
                    return_value: output,
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
                return_value: output.into_data(),
                error: None,
                gas_used,
                logs: logs
                    .into_iter()
                    .map(|log| {
                        log_index += 1;
                        reth_rpc_types::Log {
                            inner: log,
                            log_index: Some(log_index - 1),
                            transaction_index: Some(transaction_index as u64),
                            transaction_hash: Some(tx.hash()),
                            block_number: Some(block_env.number.to()),
                            block_timestamp: Some(block_env.timestamp.to()),
                            ..Default::default()
                        }
                    })
                    .collect(),
                status: true,
            },
        };

        calls.push(call);
    }

    let header = reth_primitives::Header {
        beneficiary: block_env.coinbase,
        difficulty: block_env.difficulty,
        number: block_env.number.to(),
        timestamp: block_env.timestamp.to(),
        base_fee_per_gas: Some(block_env.basefee.to()),
        gas_limit: block_env.gas_limit.to(),
        gas_used: 0,
        blob_gas_used: Some(0),
        parent_hash,
        ..Default::default()
    };

    let block = BlockWithSenders {
        block: reth_primitives::Block { header, body: transactions, ..Default::default() },
        senders,
    };

    let txs_kind =
        if full_transactions { BlockTransactionsKind::Full } else { BlockTransactionsKind::Hashes };

    let block = from_block(block, total_difficulty, txs_kind, None)?;
    Ok(SimulatedBlock { inner: block, calls })
}
