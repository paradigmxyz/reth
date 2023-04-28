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
    /// For EIP-155, EIP-2930 and Blob transactions this is set to the parity (0 for even, 1 for
    /// odd) of the y-value of the secp256k1 signature.
    ///
    /// For legacy transactions, this is the recovery id
    ///
    /// See also <https://ethereum.github.io/execution-apis/api-documentation/> and <https://ethereum.org/en/developers/docs/apis/json-rpc/#eth_gettransactionbyhash>
    pub v: U256,
}

impl Signature {
    /// Creates a new rpc signature from a [primitive signature](reth_primitives::Signature), using
    /// the give chain id to compute the signature's recovery id.
    ///
    /// If the chain id is `Some`, the recovery id is computed according to [EIP-155](https://eips.ethereum.org/EIPS/eip-155).
    pub fn from_primitive_signature(signature: PrimitiveSignature, chain_id: Option<u64>) -> Self {
        // TODO: set v value depending on tx type?
        Self { r: signature.r, s: signature.s, v: U256::from(signature.v(chain_id)) }
    }
}
