//! Signature abstraction

use alloy_primitives::{Parity, U256};

/// A signature.
pub trait Signature: Sized + Send + Sync {
    /// Decodes RLP-encoded signature, w.r.t. chain ID.
    fn decode_with_eip155_chain_id(buf: &mut &[u8]) -> alloy_rlp::Result<(Self, Option<u64>)>;
}

// todo: add optimism type that wraps Signature, to impl separately for OP to account for system
// null signature
impl Signature for alloy_primitives::Signature {
    fn decode_with_eip155_chain_id(buf: &mut &[u8]) -> alloy_rlp::Result<(Self, Option<u64>)> {
        let v: Parity = alloy_rlp::Decodable::decode(buf)?;
        let r: U256 = alloy_rlp::Decodable::decode(buf)?;
        let s: U256 = alloy_rlp::Decodable::decode(buf)?;

        if matches!(v, Parity::Parity(_)) {
            return Err(alloy_rlp::Error::Custom("invalid parity for legacy transaction"));
        }

        Ok((Self::new(r, s, v), v.chain_id()))
    }
}
