use crate::{Address, Bytes, H256};
use reth_codecs::{main_codec, Compact};
use reth_rlp::{RlpDecodable, RlpEncodable};

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

#[cfg(feature = "enable_db_speed_record")]
impl Log {
    /// Calculate size of the [Log].
    pub fn size(&self) -> usize {
        std::mem::size_of::<Address>() +
            std::mem::size_of::<H256>() * self.topics.len() +
            self.data.len()
    }
}
