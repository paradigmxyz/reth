//! EIP-7685 requests.

use alloy_consensus::Request;
use alloy_eips::eip7685::{Decodable7685, Encodable7685};
use alloy_rlp::{Decodable, Encodable};
use reth_codecs::{main_codec, Compact};
use revm_primitives::Bytes;

/// A list of EIP-7685 requests.
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
pub struct Requests(pub Vec<Request>);

impl From<Vec<Request>> for Requests {
    fn from(requests: Vec<Request>) -> Self {
        Self(requests)
    }
}

impl IntoIterator for Requests {
    type Item = Request;
    type IntoIter = std::vec::IntoIter<Request>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl Encodable for Requests {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        self.0
            .iter()
            .map(Encodable7685::encoded_7685)
            .map(Bytes::from)
            .collect::<Vec<_>>()
            .encode(out)
    }
}

impl Decodable for Requests {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        <Vec<Bytes> as Decodable>::decode(buf)?
            .into_iter()
            .map(|bytes| Request::decode_7685(&mut bytes.as_ref()))
            .collect::<Result<Vec<_>, alloy_eips::eip7685::Eip7685Error>>()
            .map(Self)
    }
}
