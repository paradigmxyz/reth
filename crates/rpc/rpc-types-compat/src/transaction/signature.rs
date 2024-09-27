use alloy_primitives::U256;
use alloy_rpc_types::{Parity, Signature};
use reth_primitives::{transaction::legacy_parity, Signature as PrimitiveSignature, TxType};

/// Creates a new rpc signature from a legacy [primitive
/// signature](reth_primitives::Signature), using the give chain id to compute the signature's
/// recovery id.
///
/// If the chain id is `Some`, the recovery id is computed according to [EIP-155](https://eips.ethereum.org/EIPS/eip-155).
pub fn from_legacy_primitive_signature(
    signature: PrimitiveSignature,
    chain_id: Option<u64>,
) -> Signature {
    Signature {
        r: signature.r(),
        s: signature.s(),
        v: U256::from(legacy_parity(&signature, chain_id).to_u64()),
        y_parity: None,
    }
}

/// Creates a new rpc signature from a non-legacy [primitive
/// signature](reth_primitives::Signature). This sets the `v` value to `0` or `1` depending on
/// the signature's `odd_y_parity`.
pub fn from_typed_primitive_signature(signature: PrimitiveSignature) -> Signature {
    Signature {
        r: signature.r(),
        s: signature.s(),
        v: U256::from(signature.v().y_parity_byte()),
        y_parity: Some(Parity(signature.v().y_parity())),
    }
}

/// Creates a new rpc signature from a legacy [primitive
/// signature](reth_primitives::Signature).
///
/// The tx type is used to determine whether or not to use the `chain_id` to compute the
/// signature's recovery id.
///
/// If the transaction is a legacy transaction, it will use the `chain_id` to compute the
/// signature's recovery id. If the transaction is a typed transaction, it will set the `v`
/// value to `0` or `1` depending on the signature's `odd_y_parity`.
pub fn from_primitive_signature(
    signature: PrimitiveSignature,
    tx_type: TxType,
    chain_id: Option<u64>,
) -> Signature {
    match tx_type {
        TxType::Legacy => from_legacy_primitive_signature(signature, chain_id),
        _ => from_typed_primitive_signature(signature),
    }
}
