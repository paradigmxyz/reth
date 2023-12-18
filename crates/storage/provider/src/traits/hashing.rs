use auto_impl::auto_impl;
use reth_db::models::BlockNumberAddress;
use reth_interfaces::provider::ProviderResult;
use reth_primitives::{Account, Address, BlockNumber, StorageEntry, B256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::{Range, RangeInclusive},
};

/// Hashing Writer
#[auto_impl(&, Arc, Box)]
pub trait HashingWriter: Send + Sync {
    /// Unwind and clear account hashing.
    ///
    /// # Returns
    ///
    /// Set of hashed keys of updated accounts.
    fn unwind_account_hashing(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account>>>;

    /// Inserts all accounts into [reth_db::tables::AccountHistory] table.
    ///
    /// # Returns
    ///
    /// Set of hashed keys of updated accounts.
    fn insert_account_for_hashing(
        &self,
        accounts: impl IntoIterator<Item = (Address, Option<Account>)>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account>>>;

    /// Unwind and clear storage hashing
    ///
    /// # Returns
    ///
    /// Mapping of hashed keys of updated accounts to their respective updated hashed slots.
    fn unwind_storage_hashing(
        &self,
        range: Range<BlockNumberAddress>,
    ) -> ProviderResult<HashMap<B256, BTreeSet<B256>>>;

    /// Iterates over storages and inserts them to hashing table.
    ///
    /// # Returns
    ///
    /// Mapping of hashed keys of updated accounts to their respective updated hashed slots.
    fn insert_storage_for_hashing(
        &self,
        storages: impl IntoIterator<Item = (Address, impl IntoIterator<Item = StorageEntry>)>,
    ) -> ProviderResult<HashMap<B256, BTreeSet<B256>>>;

    /// Calculate the hashes of all changed accounts and storages, and finally calculate the state
    /// root.
    ///
    /// The hashes are calculated from `fork_block_number + 1` to `current_block_number`.
    ///
    /// The resulting state root is compared with `expected_state_root`.
    fn insert_hashes(
        &self,
        range: RangeInclusive<BlockNumber>,
        end_block_hash: B256,
        expected_state_root: B256,
    ) -> ProviderResult<()>;
}
