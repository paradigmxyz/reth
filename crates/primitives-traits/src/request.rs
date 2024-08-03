//! EIP-7685 requests.

pub use alloy_consensus::Request;
use alloy_eips::eip7685::{Decodable7685, Encodable7685};
use alloy_rlp::{Decodable, Encodable};
use derive_more::{Deref, DerefMut, From, IntoIterator};
use reth_codecs::{reth_codec, Compact};
use revm_primitives::Bytes;
use serde::{Deserialize, Serialize};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// A list of EIP-7685 requests.
#[reth_codec]
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
)]
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
