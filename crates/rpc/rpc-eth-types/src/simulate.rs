//! Utilities for serving `eth_simulateV1`

use alloy_consensus::{TxEip4844Variant, TxType, TypedTransaction};
use alloy_primitives::Parity;
use alloy_rpc_types::{
    simulate::{SimCallResult, SimulateError, SimulatedBlock},
    Block, BlockTransactionsKind,
};
use alloy_rpc_types_eth::transaction::TransactionRequest;
use jsonrpsee_types::ErrorObject;
use reth_primitives::{
    logs_bloom,
    proofs::{calculate_receipt_root, calculate_transaction_root},
    BlockBody, BlockWithSenders, Receipt, Signature, Transaction, TransactionSigned,
    TransactionSignedNoHash,
};
use reth_revm::database::StateProviderDatabase;
use reth_rpc_server_types::result::rpc_err;
use reth_rpc_types_compat::{block::from_block, TransactionCompat};
use reth_storage_api::StateRootProvider;
use reth_trie::{HashedPostState, HashedStorage};
use revm::{db::CacheDB, Database};
use revm_primitives::{keccak256, Address, BlockEnv, Bytes, ExecutionResult, TxKind, B256, U256};

use crate::{
    cache::db::StateProviderTraitObjWrapper, error::ToRpcError, EthApiError, RevertError,
    RpcInvalidTransactionError,
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
    block_gas_limit: u64,
    chain_id: u64,
    db: &mut DB,
) -> Result<Vec<TransactionSigned>, EthApiError>
where
    EthApiError: From<DB::Error>,
{
    let mut transactions = Vec::with_capacity(txs.len());

    let default_gas_limit = {
        let total_specified_gas = txs.iter().filter_map(|tx| tx.gas).sum::<u64>();
        let txs_without_gas_limit = txs.iter().filter(|tx| tx.gas.is_none()).count();

        if total_specified_gas > block_gas_limit {
            return Err(EthApiError::Other(Box::new(EthSimulateError::BlockGasLimitExceeded)))
        }

        if txs_without_gas_limit > 0 {
            (block_gas_limit - total_specified_gas) / txs_without_gas_limit as u64
        } else {
            0
        }
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
            Signature::new(Default::default(), Default::default(), Parity::Parity(false));

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
pub fn build_block<T: TransactionCompat>(
    results: Vec<(Address, ExecutionResult)>,
    transactions: Vec<TransactionSigned>,
    block_env: &BlockEnv,
    parent_hash: B256,
    total_difficulty: U256,
    full_transactions: bool,
    db: &CacheDB<StateProviderDatabase<StateProviderTraitObjWrapper<'_>>>,
) -> Result<SimulatedBlock<Block<T::Transaction>>, EthApiError> {
    let mut calls: Vec<SimCallResult> = Vec::with_capacity(results.len());
    let mut senders = Vec::with_capacity(results.len());
    let mut receipts = Vec::new();

    let mut log_index = 0;
    for (transaction_index, ((sender, result), tx)) in
        results.into_iter().zip(transactions.iter()).enumerate()
    {
        senders.push(sender);

        let call = match result {
            ExecutionResult::Halt { reason, gas_used } => {
                let error = RpcInvalidTransactionError::halt(reason, tx.gas_limit());
                SimCallResult {
                    return_data: Bytes::new(),
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
                        alloy_rpc_types::Log {
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

        receipts.push(
            #[allow(clippy::needless_update)]
            Receipt {
                tx_type: tx.tx_type(),
                success: call.status,
                cumulative_gas_used: call.gas_used + calls.iter().map(|c| c.gas_used).sum::<u64>(),
                logs: call.logs.iter().map(|log| &log.inner).cloned().collect(),
                ..Default::default()
            }
            .into(),
        );

        calls.push(call);
    }

    let mut hashed_state = HashedPostState::default();
    for (address, account) in &db.accounts {
        let hashed_address = keccak256(address);
        hashed_state.accounts.insert(hashed_address, Some(account.info.clone().into()));

        let storage = hashed_state
            .storages
            .entry(hashed_address)
            .or_insert_with(|| HashedStorage::new(account.account_state.is_storage_cleared()));

        for (slot, value) in &account.storage {
            let slot = B256::from(*slot);
            let hashed_slot = keccak256(slot);
            storage.storage.insert(hashed_slot, *value);
        }
    }

    let state_root = db.db.0.state_root(hashed_state)?;

    let header = reth_primitives::Header {
        beneficiary: block_env.coinbase,
        difficulty: block_env.difficulty,
        number: block_env.number.to(),
        timestamp: block_env.timestamp.to(),
        base_fee_per_gas: Some(block_env.basefee.to()),
        gas_limit: block_env.gas_limit.to(),
        gas_used: calls.iter().map(|c| c.gas_used).sum::<u64>(),
        blob_gas_used: Some(0),
        parent_hash,
        receipts_root: calculate_receipt_root(&receipts),
        transactions_root: calculate_transaction_root(&transactions),
        state_root,
        logs_bloom: logs_bloom(receipts.iter().flat_map(|r| r.receipt.logs.iter())),
        mix_hash: block_env.prevrandao.unwrap_or_default(),
        ..Default::default()
    };

    let block = BlockWithSenders {
        block: reth_primitives::Block {
            header,
            body: BlockBody { transactions, ..Default::default() },
        },
        senders,
    };

    let txs_kind =
        if full_transactions { BlockTransactionsKind::Full } else { BlockTransactionsKind::Hashes };

    let block = from_block::<T>(block, total_difficulty, txs_kind, None)?;
    Ok(SimulatedBlock { inner: block, calls })
}
