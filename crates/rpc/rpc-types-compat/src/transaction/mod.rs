//! Compatibility functions for rpc `Transaction` type.

use alloy_primitives::{Address, TxKind};
use alloy_rpc_types::{
    request::{TransactionInput, TransactionRequest},
    TransactionInfo,
};
use reth_primitives::{TransactionSignedEcRecovered, TxType};
use reth_rpc_types::{Transaction, WithOtherFields};
use signature::from_primitive_signature;
pub use typed::*;

mod signature;
mod typed;

/// Create a new rpc transaction result for a mined transaction, using the given [`TransactionInfo`]
/// to populate the corresponding fields in the rpc result.
pub fn from_recovered_with_block_context(
    tx: TransactionSignedEcRecovered,
    tx_info: TransactionInfo,
) -> WithOtherFields<Transaction> {
    fill(tx, tx_info)
}

/// Create a new rpc transaction result for a _pending_ signed transaction, setting block
/// environment related fields to `None`.
pub fn from_recovered(tx: TransactionSignedEcRecovered) -> WithOtherFields<Transaction> {
    fill(tx, TransactionInfo::default())
}

/// Create a new rpc transaction result for a _pending_ signed transaction, setting block
/// environment related fields to `None`.
fn fill(
    tx: TransactionSignedEcRecovered,
    tx_info: TransactionInfo,
) -> WithOtherFields<Transaction> {
    let signer = tx.signer();
    let signed_tx = tx.into_signed();

    let to: Option<Address> = match signed_tx.kind() {
        TxKind::Create => None,
        TxKind::Call(to) => Some(Address(*to)),
    };

    #[allow(unreachable_patterns)]
    let (gas_price, max_fee_per_gas) = match signed_tx.tx_type() {
        TxType::Legacy | TxType::Eip2930 => (Some(signed_tx.max_fee_per_gas()), None),
        TxType::Eip1559 | TxType::Eip4844 | TxType::Eip7702 => {
            // the gas price field for EIP1559 is set to `min(tip, gasFeeCap - baseFee) +
            // baseFee`
            let gas_price = tx_info
                .base_fee
                .and_then(|base_fee| {
                    signed_tx.effective_tip_per_gas(Some(base_fee as u64)).map(|tip| tip + base_fee)
                })
                .unwrap_or_else(|| signed_tx.max_fee_per_gas());

            (Some(gas_price), Some(signed_tx.max_fee_per_gas()))
        }
        _ => {
            // OP-deposit
            (Some(signed_tx.max_fee_per_gas()), None)
        }
    };

    // let chain_id = signed_tx.chain_id().map(U64::from);
    let chain_id = signed_tx.chain_id();
    let blob_versioned_hashes = signed_tx.blob_versioned_hashes();
    let access_list = signed_tx.access_list().cloned();
    let authorization_list = signed_tx.authorization_list().map(|l| l.to_vec());

    let signature =
        from_primitive_signature(*signed_tx.signature(), signed_tx.tx_type(), signed_tx.chain_id());

    WithOtherFields {
        inner: Transaction {
            hash: signed_tx.hash(),
            nonce: signed_tx.nonce(),
            from: signer,
            to,
            value: signed_tx.value(),
            gas_price,
            max_fee_per_gas,
            max_priority_fee_per_gas: signed_tx.max_priority_fee_per_gas(),
            signature: Some(signature),
            gas: signed_tx.gas_limit() as u128,
            input: signed_tx.input().clone(),
            chain_id,
            access_list,
            transaction_type: Some(signed_tx.tx_type() as u8),
            // These fields are set to None because they are not stored as part of the transaction
            block_hash: tx_info.block_hash,
            block_number: tx_info.block_number,
            transaction_index: tx_info.index,
            // EIP-4844 fields
            max_fee_per_blob_gas: signed_tx.max_fee_per_blob_gas(),
            blob_versioned_hashes,
            // EIP-7702 fields
            authorization_list,
        },
        // Optimism fields
        #[cfg(feature = "optimism")]
        other: reth_rpc_types::optimism::OptimismTransactionFields {
            source_hash: signed_tx.source_hash(),
            mint: signed_tx.mint(),
            // only include is_system_tx if true: <https://github.com/ethereum-optimism/op-geth/blob/641e996a2dcf1f81bac9416cb6124f86a69f1de7/internal/ethapi/api.go#L1518-L1518>
            is_system_tx: (signed_tx.is_deposit() && signed_tx.is_system_transaction())
                .then_some(true),
            deposit_receipt_version: None,
        }
        .into(),
        #[cfg(not(feature = "optimism"))]
        other: Default::default(),
    }
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
        authorization_list,
    }
}
