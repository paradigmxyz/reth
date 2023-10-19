use crate::{Address, BlockNumber, Bloom, Bytes, B256};
use alloy_rlp::{RlpDecodable, RlpEncodable};
use reth_codecs::{main_codec, Compact};
use std::collections::BTreeMap;

/// The mapping of log address to the block numbers of blocks where the logs emitted by this
/// address occurred.
pub type LogAddressIndex = BTreeMap<Address, Vec<BlockNumber>>;

/// The mapping of log topic to the block numbers of where the topic occurred.
pub type LogTopicIndex = BTreeMap<B256, Vec<BlockNumber>>;

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
            strategy = "proptest::collection::vec(proptest::arbitrary::any::<B256>(), 0..=5)"
        )
    )]
    pub topics: Vec<B256>,
    /// Arbitrary length data.
    pub data: Bytes,
}

/// Calculate receipt logs bloom.
pub fn logs_bloom<'a, It>(logs: It) -> Bloom
where
    It: IntoIterator<Item = &'a Log>,
{
    let mut bloom = Bloom::ZERO;
    for log in logs {
        bloom.m3_2048(log.address.as_slice());
        for topic in &log.topics {
            bloom.m3_2048(topic.as_slice());
        }
    }
    bloom
}
