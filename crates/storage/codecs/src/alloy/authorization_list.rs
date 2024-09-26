use core::ops::Deref;

use crate::Compact;
use alloy_eips::eip7702::{Authorization as AlloyAuthorization, SignedAuthorization};
use alloy_primitives::{Address, U256};
use bytes::Buf;
use reth_codecs_derive::add_arbitrary_tests;

/// Authorization acts as bridge which simplifies Compact implementation for AlloyAuthorization.
///
/// Notice: Make sure this struct is 1:1 with `alloy_eips::eip7702::Authorization`
#[derive(Debug, Clone, PartialEq, Eq, Default, Compact)]
#[cfg_attr(test, derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize))]
#[add_arbitrary_tests(compact)]
pub(crate) struct Authorization {
    chain_id: U256,
    address: Address,
    nonce: u64,
}

impl Compact for AlloyAuthorization {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let authorization =
            Authorization { chain_id: self.chain_id, address: self.address, nonce: self.nonce() };
        authorization.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (authorization, buf) = Authorization::from_compact(buf, len);
        let alloy_authorization = Self {
            chain_id: authorization.chain_id,
            address: authorization.address,
            nonce: authorization.nonce,
        };
        (alloy_authorization, buf)
    }
}

impl Compact for SignedAuthorization {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let signature = self.signature();
        let (v, r, s) = (signature.v(), signature.r(), signature.s());
        buf.put_u8(v.y_parity_byte());
        buf.put_slice(r.as_le_slice());
        buf.put_slice(s.as_le_slice());

        // to_compact doesn't write the len to buffer.
        // By placing it as last, we don't need to store it either.
        1 + 32 + 32 + self.deref().to_compact(buf)
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        let y = alloy_primitives::Parity::Parity(buf.get_u8() == 1);
        let r = U256::from_le_slice(&buf[0..32]);
        buf.advance(32);
        let s = U256::from_le_slice(&buf[0..32]);
        buf.advance(32);

        let signature = alloy_primitives::Signature::from_rs_and_parity(r, s, y)
            .expect("invalid authorization signature");
        let (auth, buf) = AlloyAuthorization::from_compact(buf, len);

        (auth.into_signed(signature), buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};

    #[test]
    fn test_roundtrip_compact_authorization_list_item() {
        let authorization = AlloyAuthorization {
            chain_id: U256::from(1),
            address: address!("dac17f958d2ee523a2206206994597c13d831ec7"),
            nonce: 1,
        }
        .into_signed(
            alloy_primitives::Signature::from_rs_and_parity(
                b256!("1fd474b1f9404c0c5df43b7620119ffbc3a1c3f942c73b6e14e9f55255ed9b1d").into(),
                b256!("29aca24813279a901ec13b5f7bb53385fa1fc627b946592221417ff74a49600d").into(),
                false,
            )
            .unwrap(),
        );
        let mut compacted_authorization = Vec::<u8>::new();
        let len = authorization.to_compact(&mut compacted_authorization);
        let (decoded_authorization, _) =
            SignedAuthorization::from_compact(&compacted_authorization, len);
        assert_eq!(authorization, decoded_authorization);
    }
}
