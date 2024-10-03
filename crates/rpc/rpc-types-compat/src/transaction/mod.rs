//! Compatibility functions for rpc `Transaction` type.
mod signature;

pub use signature::*;
use std::fmt;

use alloy_rpc_types::{
    request::{TransactionInput, TransactionRequest},
    Transaction, TransactionInfo,
};
use alloy_serde::WithOtherFields;
use reth_primitives::{TransactionSigned, TransactionSignedEcRecovered, TxType};

/// Create a new rpc transaction result for a mined transaction, using the given block hash,
/// number, and tx index fields to populate the corresponding fields in the rpc result.
///
/// The block hash, number, and tx index fields should be from the original block where the
/// transaction was mined.
pub fn from_recovered_with_block_context<T: TransactionCompat>(
    tx: TransactionSignedEcRecovered,
    tx_info: TransactionInfo,
) -> T::Transaction {
    T::fill(tx, tx_info)
}

/// Create a new rpc transaction result for a _pending_ signed transaction, setting block
/// environment related fields to `None`.
pub fn from_recovered<T: TransactionCompat>(tx: TransactionSignedEcRecovered) -> T::Transaction {
    T::fill(tx, TransactionInfo::default())
}

/// Builds RPC transaction w.r.t. network.
pub trait TransactionCompat: Send + Sync + Unpin + Clone + fmt::Debug {
    /// RPC transaction response type.
    type Transaction: Send + Clone + Default + fmt::Debug;

    /// Formats gas price and max fee per gas for RPC transaction response w.r.t. network specific
    /// transaction type.
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
    fn fill(tx: TransactionSignedEcRecovered, tx_inf: TransactionInfo) -> Self::Transaction;

    /// Truncates the input of a transaction to only the first 4 bytes.
    // todo: remove in favour of using constructor on `TransactionResponse` or similar
    // <https://github.com/alloy-rs/alloy/issues/1315>.
    fn otterscan_api_truncate_input(tx: &mut Self::Transaction);

    /// Returns the transaction type.
    // todo: remove when alloy TransactionResponse trait it updated.
    fn tx_type(tx: &Self::Transaction) -> u8;
}

impl TransactionCompat for () {
    // this noop impl depends on integration in `reth_rpc_eth_api::EthApiTypes` noop impl, and
    // `alloy_network::AnyNetwork`
    type Transaction = WithOtherFields<Transaction>;

    fn fill(_tx: TransactionSignedEcRecovered, _tx_info: TransactionInfo) -> Self::Transaction {
        WithOtherFields::default()
    }

    fn otterscan_api_truncate_input(_tx: &mut Self::Transaction) {}

    fn tx_type(_tx: &Self::Transaction) -> u8 {
        0
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
    let authorization_list = tx.transaction.authorization_list().map(|l| l.to_vec());
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
        gas: Some(gas),
        value: Some(value),
        input: TransactionInput::new(input),
        nonce: Some(nonce),
        chain_id,
        access_list,
        max_fee_per_blob_gas,
        blob_versioned_hashes,
        transaction_type: Some(tx_type.into()),
        sidecar: None,
        authorization_list,
    }
}
