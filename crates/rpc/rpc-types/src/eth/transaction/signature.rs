//! Signature related RPC values
use reth_primitives::{Signature as PrimitiveSignature, U256};
use serde::{Deserialize, Serialize};

/// Container type for all signature fields in RPC
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Signature {
    /// The R field of the signature; the point on the curve.
    pub r: U256,
    /// The S field of the signature; the point on the curve.
    pub s: U256,
    // todo: not just 0 or 1, due to eip155
    /// The standardised recovery id of the signature (0 or 1).
    pub v: U256,
}

impl Signature {
    /// Creates a new rpc signature from a [primitive signature](reth_primitives::Signature), using
    /// the give chain id to compute the signature's recovery id.
    ///
    /// If the chain id is `Some`, the recovery id is computed according to [EIP-155](https://eips.ethereum.org/EIPS/eip-155).
    pub fn from_primitive_signature(signature: PrimitiveSignature, chain_id: Option<u64>) -> Self {
        Self { r: signature.r, s: signature.s, v: U256::from(signature.v(chain_id)) }
    }
}
