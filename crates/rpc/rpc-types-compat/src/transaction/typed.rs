/// Converts a typed transaction request into a primitive transaction.
///
/// Returns `None` if any of the following are true:
/// - `nonce` is greater than [`u64::MAX`]
/// - `gas_limit` is greater than [`u64::MAX`]
/// - `value` is greater than [`u128::MAX`]
pub fn to_primitive_transaction(
    tx_request: reth_rpc_types::TypedTransactionRequest,
) -> Option<reth_primitives::Transaction> {
    use reth_primitives::{Transaction, TxEip1559, TxEip2930, TxEip4844, TxLegacy};
    use reth_rpc_types::TypedTransactionRequest;

    Some(match tx_request {
        TypedTransactionRequest::Legacy(tx) => Transaction::Legacy(TxLegacy {
            chain_id: tx.chain_id,
            nonce: tx.nonce.to(),
            gas_price: tx.gas_price.to(),
            gas_limit: tx.gas_limit.try_into().ok()?,
            to: to_primitive_transaction_kind(tx.kind),
            value: tx.value,
            input: tx.input,
        }),
        TypedTransactionRequest::EIP2930(tx) => Transaction::Eip2930(TxEip2930 {
            chain_id: tx.chain_id,
            nonce: tx.nonce.to(),
            gas_price: tx.gas_price.to(),
            gas_limit: tx.gas_limit.try_into().ok()?,
            to: to_primitive_transaction_kind(tx.kind),
            value: tx.value,
            input: tx.input,
            access_list: tx.access_list.into(),
        }),
        TypedTransactionRequest::EIP1559(tx) => Transaction::Eip1559(TxEip1559 {
            chain_id: tx.chain_id,
            nonce: tx.nonce.to(),
            max_fee_per_gas: tx.max_fee_per_gas.to(),
            gas_limit: tx.gas_limit.try_into().ok()?,
            to: to_primitive_transaction_kind(tx.kind),
            value: tx.value,
            input: tx.input,
            access_list: tx.access_list.into(),
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas.to(),
        }),
        TypedTransactionRequest::EIP4844(tx) => Transaction::Eip4844(TxEip4844 {
            chain_id: tx.chain_id,
            nonce: tx.nonce.to(),
            gas_limit: tx.gas_limit.to(),
            max_fee_per_gas: tx.max_fee_per_gas.to(),
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas.to(),
            to: to_primitive_transaction_kind(tx.kind),
            value: tx.value,
            access_list: tx.access_list.into(),
            blob_versioned_hashes: tx.blob_versioned_hashes,
            max_fee_per_blob_gas: tx.max_fee_per_blob_gas.to(),
            input: tx.input,
        }),
    })
}

/// Transforms a [reth_rpc_types::TransactionKind] into a [reth_primitives::TransactionKind]
pub fn to_primitive_transaction_kind(
    kind: reth_rpc_types::TransactionKind,
) -> reth_primitives::TransactionKind {
    match kind {
        reth_rpc_types::TransactionKind::Call(to) => reth_primitives::TransactionKind::Call(to),
        reth_rpc_types::TransactionKind::Create => reth_primitives::TransactionKind::Create,
    }
}
