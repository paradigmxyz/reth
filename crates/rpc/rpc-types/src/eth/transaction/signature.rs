//! Signature related RPC values
use reth_primitives::{Signature as PrimitiveSignature, TxType, U256};
use serde::{Deserialize, Serialize};

/// Container type for all signature fields in RPC
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Signature {
    /// The R field of the signature; the point on the curve.
    pub r: U256,
    /// The S field of the signature; the point on the curve.
    pub s: U256,
    /// For EIP-155, EIP-2930 and Blob transactions this is set to the parity (0 for even, 1 for
    /// odd) of the y-value of the secp256k1 signature.
    ///
    /// For legacy transactions, this is the recovery id
    ///
    /// See also <https://ethereum.github.io/execution-apis/api-documentation/> and <https://ethereum.org/en/developers/docs/apis/json-rpc/#eth_gettransactionbyhash>
    pub v: U256,
}

impl Signature {
    /// Creates a new rpc signature from a legacy [primitive
    /// signature](reth_primitives::Signature), using the give chain id to compute the signature's
    /// recovery id.
    ///
    /// If the chain id is `Some`, the recovery id is computed according to [EIP-155](https://eips.ethereum.org/EIPS/eip-155).
    pub(crate) fn from_legacy_primitive_signature(
        signature: PrimitiveSignature,
        chain_id: Option<u64>,
    ) -> Self {
        Self { r: signature.r, s: signature.s, v: U256::from(signature.v(chain_id)) }
    }

    /// Creates a new rpc signature from a non-legacy [primitive
    /// signature](reth_primitives::Signature). This sets the `v` value to `0` or `1` depending on
    /// the signature's `odd_y_parity`.
    pub(crate) fn from_typed_primitive_signature(signature: PrimitiveSignature) -> Self {
        Self { r: signature.r, s: signature.s, v: U256::from(signature.odd_y_parity as u8) }
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
    ) -> Self {
        match tx_type {
            TxType::Legacy => Signature::from_legacy_primitive_signature(signature, chain_id),
            _ => Signature::from_typed_primitive_signature(signature),
        }
    }
}
