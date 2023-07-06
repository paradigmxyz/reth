use crate::{Address, BlockNumber, Bytes, H256};
use reth_codecs::{main_codec, Compact};
use reth_rlp::{RlpDecodable, RlpEncodable};
use std::collections::BTreeMap;

/// The mapping of log address to block number indices (blocks where the logs emitted by this
/// address occurred).
pub type LogAddressIndices = BTreeMap<Address, Vec<BlockNumber>>;

/// The mapping of log topic to block number indices (blocks where the topic occurred).
pub type LogTopicIndices = BTreeMap<H256, Vec<BlockNumber>>;

/// Ethereum Log
#[main_codec(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpDecodable, RlpEncodable, Default)]
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
