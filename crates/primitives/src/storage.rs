use super::{H256, U256};
use reth_codecs::{main_codec, Compact};

/// Account storage entry.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[main_codec]
pub struct StorageEntry {
    /// Storage key.
    pub key: H256,
    /// Value on storage key.
    pub value: U256,
}
