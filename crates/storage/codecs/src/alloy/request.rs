//! Native Compact codec impl for EIP-7685 requests.

use crate::Compact;
use alloy_consensus::Request;
use alloy_eips::eip7685::{Decodable7685, Encodable7685};
use alloy_primitives::Bytes;
use bytes::BufMut;

impl Compact for Request {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        let encoded: Bytes = self.encoded_7685().into();
        encoded.to_compact(buf)
    }

    fn from_compact(buf: &[u8], _: usize) -> (Self, &[u8]) {
        let (raw, buf) = Bytes::from_compact(buf, buf.len());

        (Self::decode_7685(&mut raw.as_ref()).expect("invalid eip-7685 request in db"), buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::proptest;
    use proptest_arbitrary_interop::arb;

    proptest! {
        #[test]
        fn roundtrip(request in arb::<Request>()) {
            let mut buf = Vec::<u8>::new();
            request.to_compact(&mut buf);
            let (decoded, _) = Request::from_compact(&buf, buf.len());
            assert_eq!(request, decoded);
        }
    }
}
