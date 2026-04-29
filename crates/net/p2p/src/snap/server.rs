use alloy_primitives::{Bytes, B256};
use reth_eth_wire_types::{
    snap::{AccountData, StorageData},
    BlockAccessLists,
};

/// Provides state and BAL data for serving snap protocol requests.
pub trait SnapStateProvider: Send + Sync + 'static {
    /// Iterates accounts in hash-sorted order from `starting_hash` up to, but not including,
    /// `limit_hash`. Returns at most `response_bytes` worth of data.
    ///
    /// The second return value contains boundary proof nodes when available.
    fn account_range(
        &self,
        root_hash: B256,
        starting_hash: B256,
        limit_hash: B256,
        response_bytes: u64,
    ) -> (Vec<AccountData>, Vec<Bytes>);

    /// Iterates storage slots for the given account hashes.
    ///
    /// The second return value contains boundary proof nodes when available.
    fn storage_ranges(
        &self,
        root_hash: B256,
        account_hashes: Vec<B256>,
        starting_hash: B256,
        limit_hash: B256,
        response_bytes: u64,
    ) -> (Vec<Vec<StorageData>>, Vec<Bytes>);

    /// Returns bytecodes for the given code hashes.
    fn bytecodes(&self, hashes: Vec<B256>, response_bytes: u64) -> Vec<Bytes>;

    /// Returns snap/2 BAL response entries for the given block hashes.
    ///
    /// Missing BALs must be encoded as the RLP empty string (`0x80`) for snap/2.
    fn block_access_lists(&self, block_hashes: Vec<B256>, response_bytes: u64) -> BlockAccessLists;
}
