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
            nonce: tx.nonce,
            gas_price: tx.gas_price.to(),
            gas_limit: tx.gas_limit.try_into().ok()?,
            to: tx.kind,
            value: tx.value,
            input: tx.input,
        }),
        TypedTransactionRequest::EIP2930(tx) => Transaction::Eip2930(TxEip2930 {
            chain_id: tx.chain_id,
            nonce: tx.nonce,
            gas_price: tx.gas_price.to(),
            gas_limit: tx.gas_limit.try_into().ok()?,
            to: tx.kind,
            value: tx.value,
            input: tx.input,
            access_list: tx.access_list,
        }),
        TypedTransactionRequest::EIP1559(tx) => Transaction::Eip1559(TxEip1559 {
            chain_id: tx.chain_id,
            nonce: tx.nonce,
            max_fee_per_gas: tx.max_fee_per_gas.to(),
            gas_limit: tx.gas_limit.try_into().ok()?,
            to: tx.kind,
            value: tx.value,
            input: tx.input,
            access_list: tx.access_list,
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas.to(),
        }),
        TypedTransactionRequest::EIP4844(tx) => Transaction::Eip4844(TxEip4844 {
            chain_id: tx.chain_id,
            nonce: tx.nonce,
            gas_limit: tx.gas_limit.to(),
            max_fee_per_gas: tx.max_fee_per_gas.to(),
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas.to(),
            to: tx.kind,
            value: tx.value,
            access_list: tx.access_list,
            blob_versioned_hashes: tx.blob_versioned_hashes,
            max_fee_per_blob_gas: tx.max_fee_per_blob_gas.to(),
            input: tx.input,
        }),
    })
}
