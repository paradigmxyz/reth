use super::{H256, U256};
use bytes::Buf;
use modular_bitfield::prelude::*;
use reth_codecs::{use_compact, Compact};

/// Account storage entry.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[use_compact]
pub struct StorageEntry {
    /// Storage key.
    pub key: H256,
    /// Value on storage key.
    pub value: U256,
}
