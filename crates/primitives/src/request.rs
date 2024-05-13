//! EIP-7685 requests.

use alloy_consensus::Request;
use reth_codecs::{main_codec, Compact};

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
