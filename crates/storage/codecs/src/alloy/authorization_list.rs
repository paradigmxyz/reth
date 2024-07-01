use crate::Compact;
use alloy_eips::eip7702::Authorization;
use alloy_primitives::Address;
use bytes::{Buf, BufMut};

// todo: signed auth
impl Compact for Authorization {
    // TODO(eip7702): actually compact this
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut buffer = bytes::BytesMut::new();
        buffer.put_u64(self.chain_id);
        buffer.put_slice(&self.address.0.as_slice());
        if self.nonce.is_some() {
            buffer.put_u8(1);
            buffer.put_u64(self.nonce.unwrap());
        } else {
            buffer.put_u8(0);
        }
        /*buffer.put_u8(self.y_parity as u8);
        buffer.put_slice(&self.r.as_le_slice());
        buffer.put_slice(&self.s.as_le_slice());*/
        let total_length = buffer.len();
        buf.put(buffer);
        total_length
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let chain_id = bytes::Buf::get_u64(&mut buf);
        let address = Address::from_slice(&buf[0..20]);
        buf.advance(20);
        let has_nonce = bytes::Buf::get_u8(&mut buf);
        let nonce = if has_nonce == 1 {
            let nonce = bytes::Buf::get_u64(&mut buf);
            Some(nonce)
        } else {
            None
        };
        let authorization = Authorization { chain_id, address, nonce: nonce.into() };
        (authorization, buf)
    }
}

// TODO(eip7702): complete these tests
#[cfg(test)]
mod tests {
    /*
    use super::*;
    use alloy_primitives::{address, b256};

    #[test]
    fn test_roundtrip_compact_authorization_list_item() {
        let authorization = Authorization {
            chain_id: 1,
            address: address!("dac17f958d2ee523a2206206994597c13d831ec7"),
            nonce: None,
            y_parity: false,
            r: b256!("1fd474b1f9404c0c5df43b7620119ffbc3a1c3f942c73b6e14e9f55255ed9b1d").into(),
            s: b256!("29aca24813279a901ec13b5f7bb53385fa1fc627b946592221417ff74a49600d").into(),
        };
        let mut compacted_authorization = Vec::<u8>::new();
        let len = authorization.clone().to_compact(&mut compacted_authorization);
        let (decoded_authorization, _) = Authorization::from_compact(&compacted_authorization, len);
        assert_eq!(authorization, decoded_authorization);
        }*/
}
