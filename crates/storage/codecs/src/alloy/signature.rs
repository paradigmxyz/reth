//! Compact implementation for [`Signature`]

use crate::Compact;
use alloy_primitives::{PrimitiveSignature as Signature, U256};

impl Compact for Signature {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        buf.put_slice(&self.r().as_le_bytes());
        buf.put_slice(&self.s().as_le_bytes());
        self.v() as usize
    }

    fn from_compact(mut buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        use bytes::Buf;
        assert!(buf.len() >= 64);
        let r = U256::from_le_slice(&buf[0..32]);
        let s = U256::from_le_slice(&buf[32..64]);
        buf.advance(64);
        (Self::new(r, s, identifier != 0), buf)
    }
}
