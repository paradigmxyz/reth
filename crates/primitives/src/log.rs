use crate::{rpc::Log as RpcLog, Address, Bytes, H256};
use reth_codecs::{main_codec, Compact};
use reth_rlp::{RlpDecodable, RlpEncodable};

/// Ethereum Log
#[main_codec(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpDecodable, RlpEncodable, Default)]
pub struct Log {
    /// Contract that emitted this log.
    pub address: Address,
    /// Topics of the log. The number of logs depend on what `LOG` opcode is used.
    pub topics: Vec<H256>,
    /// Arbitrary length data.
    pub data: Bytes,
}

impl From<Log> for RpcLog {
    fn from(log: Log) -> Self {
        Self {
            address: log.address.into(),
            topics: log.topics.into_iter().map(Into::into).collect(),
            data: log.data.to_vec().into(),
            block_hash: None,
            block_number: None,
            transaction_hash: None,
            transaction_index: None,
            log_index: None,
            transaction_log_index: None,
            log_type: None,
            removed: None,
        }
    }
}
