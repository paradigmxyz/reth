//! Native Compact codec impl for EIP-7685 requests.

use alloy_consensus::Request;
use alloy_eips::eip7685::{Decodable7685, Encodable7685};
use alloy_primitives::Bytes;
use bytes::BufMut;
use alloc::vec::Vec;
use alloy_rlp::{Decodable, Encodable};
use derive_more::{Deref, DerefMut, From, IntoIterator};
use crate::{add_arbitrary_tests, Compact};
use serde::{Deserialize, Serialize};


/// A list of EIP-7685 requests.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Default,
    Hash,
    Deref,
    DerefMut,
    From,
    IntoIterator,
    Serialize,
    Deserialize,
    Compact
)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct Requests(pub Vec<Request>);

impl Encodable for Requests {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        let mut h = alloy_rlp::Header { list: true, payload_length: 0 };

        let mut encoded = Vec::new();
        for req in &self.0 {
            let encoded_req = req.encoded_7685();
            h.payload_length += encoded_req.len();
            encoded.push(Bytes::from(encoded_req));
        }

        h.encode(out);
        for req in encoded {
            req.encode(out);
        }
    }
}

impl Decodable for Requests {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(<Vec<Bytes> as Decodable>::decode(buf)?
            .into_iter()
            .map(|bytes| Request::decode_7685(&mut bytes.as_ref()))
            .collect::<Result<Vec<_>, alloy_eips::eip7685::Eip7685Error>>()
            .map(Self)?)
    }
}



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
    use core::default;

    use crate::alloy::request;

    use super::*;
    use alloy_eips::eip6110::DepositRequest;
    use proptest::proptest;
    use proptest_arbitrary_interop::arb;
     
        #[test]
        fn requests_roundtrip() {
            let re=alloy_consensus::Request::DepositRequest(DepositRequest::default());
            let reqs=vec![re,re,re,re];
            let mut buf = Vec::<u8>::new();
            reqs.to_compact(&mut buf);
            
            let (decoded, _) = Requests::from_compact(&buf, buf.len());
            assert_eq!(reqs, decoded.0);
        }
    

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
