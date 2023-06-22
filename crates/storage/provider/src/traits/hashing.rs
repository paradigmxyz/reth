use auto_impl::auto_impl;
use reth_db::models::BlockNumberAddress;
use reth_interfaces::Result;
use reth_primitives::{Address, BlockNumber, StorageEntry, H256};
use std::ops::{Range, RangeInclusive};

/// Hashing Writer
#[auto_impl(&, Arc, Box)]
pub trait HashingWriter: Send + Sync {
    /// Unwind and clear storage hashing
    fn unwind_storage_hashing(&self, range: Range<BlockNumberAddress>) -> Result<()>;

    /// iterate over storages and insert them to hashing table
    fn insert_storage_for_hashing(
        &self,
        storages: impl IntoIterator<Item = (Address, impl IntoIterator<Item = StorageEntry>)>,
    ) -> Result<()>;

    /// Calculate the hashes of all changed accounts and storages, and finally calculate the state
    /// root.
    ///
    /// The hashes are calculated from `fork_block_number + 1` to `current_block_number`.
    ///
    /// The resulting state root is compared with `expected_state_root`.
    fn insert_hashes(
        &self,
        range: RangeInclusive<BlockNumber>,
        end_block_hash: H256,
        expected_state_root: H256,
    ) -> Result<()>;
}
