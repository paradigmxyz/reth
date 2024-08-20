//! Compatibility functions for rpc `Transaction` type.

pub use signature::*;
pub use typed::*;

mod signature;
mod typed;

use std::fmt;

use alloy_rpc_types::request::{TransactionInput, TransactionRequest};
use reth_primitives::{BlockNumber, TransactionSigned, TransactionSignedEcRecovered, TxType, B256};
use reth_rpc_types::{Transaction, WithOtherFields};

/// Builds RPC transaction w.r.t. network.
pub trait TransactionCompat: Send + Sync + Unpin + Clone + fmt::Debug {
    /// RPC transaction response type.
    type Transaction: Send + Clone + Default + fmt::Debug;

    /// Returns gas price and max fee per gas w.r.t. network specific transaction type.
    fn gas_price(signed_tx: &TransactionSigned, base_fee: Option<u64>) -> GasPrice {
        match signed_tx.tx_type() {
            TxType::Legacy | TxType::Eip2930 => {
                GasPrice { gas_price: Some(signed_tx.max_fee_per_gas()), max_fee_per_gas: None }
            }
            TxType::Eip1559 | TxType::Eip4844 => {
                // the gas price field for EIP1559 is set to `min(tip, gasFeeCap - baseFee) +
                // baseFee`
                let gas_price = base_fee
                    .and_then(|base_fee| {
                        signed_tx
                            .effective_tip_per_gas(Some(base_fee))
                            .map(|tip| tip + base_fee as u128)
                    })
                    .unwrap_or_else(|| signed_tx.max_fee_per_gas());

                GasPrice {
                    gas_price: Some(gas_price),
                    max_fee_per_gas: Some(signed_tx.max_fee_per_gas()),
                }
            }
            _ => GasPrice::default(),
        }
    }

    /// Create a new rpc transaction result for a _pending_ signed transaction, setting block
    /// environment related fields to `None`.
    fn fill(
        tx: TransactionSignedEcRecovered,
        block_hash: Option<B256>,
        block_number: Option<BlockNumber>,
        base_fee: Option<u64>,
        transaction_index: Option<usize>,
    ) -> Self::Transaction;

    /// Create a new rpc transaction result for a _pending_ signed transaction, setting block
    /// environment related fields to `None`.
    fn from_recovered(tx: TransactionSignedEcRecovered) -> Self::Transaction {
        Self::fill(tx, None, None, None, None)
    }

    /// Create a new rpc transaction result for a mined transaction, using the given block hash,
    /// number, and tx index fields to populate the corresponding fields in the rpc result.
    ///
    /// The block hash, number, and tx index fields should be from the original block where the
    /// transaction was mined.
    fn from_recovered_with_block_context(
        tx: TransactionSignedEcRecovered,
        block_hash: B256,
        block_number: BlockNumber,
        base_fee: Option<u64>,
        tx_index: usize,
    ) -> Self::Transaction {
        Self::fill(tx, Some(block_hash), Some(block_number), base_fee, Some(tx_index))
    }
}

/// Gas price and max fee per gas for a transaction. Helper type to format transaction RPC response.
#[derive(Debug, Default)]
pub struct GasPrice {
    /// Gas price for transaction.
    pub gas_price: Option<u128>,
    /// Max fee per gas for transaction.
    pub max_fee_per_gas: Option<u128>,
}

/// Convert [`TransactionSignedEcRecovered`] to [`TransactionRequest`]
pub fn transaction_to_call_request(tx: TransactionSignedEcRecovered) -> TransactionRequest {
    let from = tx.signer();
    let to = Some(tx.transaction.to().into());
    let gas = tx.transaction.gas_limit();
    let value = tx.transaction.value();
    let input = tx.transaction.input().clone();
    let nonce = tx.transaction.nonce();
    let chain_id = tx.transaction.chain_id();
    let access_list = tx.transaction.access_list().cloned();
    let max_fee_per_blob_gas = tx.transaction.max_fee_per_blob_gas();
    let _authorization_list = tx.transaction.authorization_list();
    let blob_versioned_hashes = tx.transaction.blob_versioned_hashes();
    let tx_type = tx.transaction.tx_type();

    // fees depending on the transaction type
    let (gas_price, max_fee_per_gas) = if tx.is_dynamic_fee() {
        (None, Some(tx.max_fee_per_gas()))
    } else {
        (Some(tx.max_fee_per_gas()), None)
    };
    let max_priority_fee_per_gas = tx.transaction.max_priority_fee_per_gas();

    TransactionRequest {
        from: Some(from),
        to,
        gas_price,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        gas: Some(gas as u128),
        value: Some(value),
        input: TransactionInput::new(input),
        nonce: Some(nonce),
        chain_id,
        access_list,
        max_fee_per_blob_gas,
        blob_versioned_hashes,
        transaction_type: Some(tx_type.into()),
        sidecar: None,
    }
}

impl TransactionCompat for () {
    // this noop impl depends on integration in `reth_rpc_eth_api::EthApiTypes` noop impl, and
    // `alloy_network::AnyNetwork`
    type Transaction = WithOtherFields<Transaction>;

    fn fill(
        _tx: TransactionSignedEcRecovered,
        _block_hash: Option<B256>,
        _block_number: Option<BlockNumber>,
        _base_fee: Option<u64>,
        _transaction_index: Option<usize>,
    ) -> Self::Transaction {
        WithOtherFields::default()
    }
}
