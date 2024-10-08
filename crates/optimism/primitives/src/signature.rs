//! Optimism wrapper of [`Signature`]

use alloy_primitives::{Parity, Signature, U256};
use derive_more::{Deref, From};
use serde::{Deserialize, Serialize};

/// Optimism wrapper for [`Signature`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, From, Deref, Serialize, Deserialize)]
pub struct OpSignature {
    #[serde(flatten)]
    inner: Signature,
}

impl OpSignature {
    /// Returns a new OP wrapper of [`Signature`].
    pub fn new(r: U256, s: U256, v: Parity) -> Self {
        Self { inner: Signature::new(r, s, v) }
    }

    /// Returns the signature for the optimism deposit transactions, which don't include a
    /// signature.
    pub fn optimism_deposit_tx_signature() -> Self {
        Self::new(U256::ZERO, U256::ZERO, Parity::Parity(false))
    }

    /// Returns `true` if signature is op system null signature.
    fn is_optimism_deposit_tx_signature(&self) -> bool {
        !self.v().y_parity() && self.r().is_zero() && self.s().is_zero()
    }
}

impl reth_primitives_traits::Signature for OpSignature {
    fn r(&self) -> U256 {
        self.deref().r()
    }

    fn s(&self) -> U256 {
        self.deref().s()
    }

    fn v(&self) -> Parity {
        self.deref().v()
    }

    fn decode_with_eip155_chain_id(buf: &mut &[u8]) -> alloy_rlp::Result<(Self, Option<u64>)> {
        let v: Parity = alloy_rlp::Decodable::decode(buf)?;
        let r: U256 = alloy_rlp::Decodable::decode(buf)?;
        let s: U256 = alloy_rlp::Decodable::decode(buf)?;

        // pre bedrock system transactions were sent from the zero address as legacy
        // transactions with an empty signature
        //
        // NOTE: this is very hacky and only relevant for op-mainnet pre bedrock
        if matches!(v, Parity::Parity(false)) && r.is_zero() && s.is_zero() {
            return Ok((Self::new(r, s, Parity::Parity(false)), None));
        }

        Ok((Self::new(r, s, v), v.chain_id()))
    }

    fn legacy_parity(&self, chain_id: Option<u64>) -> Parity {
        if let Some(chain_id) = chain_id {
            Parity::Parity(self.v().y_parity()).with_chain_id(chain_id)
        } else {
            // pre bedrock system transactions were sent from the zero address as legacy
            // transactions with an empty signature
            //
            // NOTE: this is very hacky and only relevant for op-mainnet pre bedrock
            if self.is_optimism_deposit_tx_signature() {
                return Parity::Parity(false);
            }
            Parity::NonEip155(self.v().y_parity())
        }
    }

    fn with_eip155_parity(&self, chain_id: Option<u64>) -> Self {
        Self { inner: Signature::with_eip155_parity(&self.inner, chain_id) }
    }

    #[inline]
    fn extract_chain_id(v: u64) -> alloy_rlp::Result<(bool, Option<u64>)> {
        Signature::extract_chain_id(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives_traits::Signature as _;

    #[test]
    fn test_legacy_parity() {
        // Select 1 as an arbitrary nonzero value for R and S, as v() always returns 0 for (0, 0).
        let signature = OpSignature::new(U256::from(1), U256::from(1), Parity::Parity(false));

        assert_eq!(Parity::NonEip155(false), signature.legacy_parity(None));
        assert_eq!(Parity::Eip155(37), signature.legacy_parity(Some(1)));

        let signature = OpSignature::new(U256::from(1), U256::from(1), Parity::Parity(true));

        assert_eq!(Parity::NonEip155(true), signature.legacy_parity(None));
        assert_eq!(Parity::Eip155(38), signature.legacy_parity(Some(1)));
    }
}
