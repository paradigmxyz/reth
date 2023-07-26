use crate::{Address, Bytes, H256};
use alloy_rlp::{RlpDecodable, RlpEncodable};
use reth_codecs::{main_codec, Compact};

/// Ethereum Log
#[main_codec(rlp)]
#[derive(Clone, Debug, Default, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct Log {
    /// Contract that emitted this log.
    pub address: Address,
    /// Topics of the log. The number of logs depend on what `LOG` opcode is used.
    #[cfg_attr(
        any(test, feature = "arbitrary"),
        proptest(
            strategy = "proptest::collection::vec(proptest::arbitrary::any::<H256>(), 0..=5)"
        )
    )]
    pub topics: Vec<H256>,
    /// Arbitrary length data.
    pub data: Bytes,
}
