use alloc::collections::{BTreeMap, BTreeSet};
use alloy_primitives::{map::B256Map, Address, BlockNumber, B256};
use core::ops::RangeBounds;
use reth_db_api::models::BlockNumberAddress;
use reth_db_models::AccountBeforeTx;
use reth_primitives_traits::{Account, StorageEntry};
use reth_storage_errors::provider::ProviderResult;

/// Hashing Writer
pub trait HashingWriter: crate::AccountExtensionProvider + Send {
    /// Unwind and clear account hashing.
    ///
    /// # Returns
    ///
    /// Set of hashed keys of updated accounts.
    fn unwind_account_hashing<'a>(
        &self,
        changesets: impl Iterator<Item = &'a (BlockNumber, AccountBeforeTx<Self::AccountExtension>)>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account<Self::AccountExtension>>>>;

    /// Unwind and clear account hashing in a given block range.
    ///
    /// # Returns
    ///
    /// Set of hashed keys of updated accounts.
    fn unwind_account_hashing_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account<Self::AccountExtension>>>>;

    /// Inserts all accounts into [`AccountsHistory`][reth_db_api::tables::AccountsHistory] table.
    ///
    /// # Returns
    ///
    /// Set of hashed keys of updated accounts.
    fn insert_account_for_hashing(
        &self,
        accounts: impl IntoIterator<Item = (Address, Option<Account<Self::AccountExtension>>)>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account<Self::AccountExtension>>>>;

    /// Unwind and clear storage hashing.
    ///
    /// # Returns
    ///
    /// Mapping of hashed keys of updated accounts to their respective updated hashed slots.
    fn unwind_storage_hashing(
        &self,
        changesets: impl Iterator<Item = (BlockNumberAddress, StorageEntry)>,
    ) -> ProviderResult<B256Map<BTreeSet<B256>>>;

    /// Unwind and clear storage hashing in a given block range.
    ///
    /// # Returns
    ///
    /// Mapping of hashed keys of updated accounts to their respective updated hashed slots.
    fn unwind_storage_hashing_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<B256Map<BTreeSet<B256>>>;

    /// Iterates over storages and inserts them to hashing table.
    ///
    /// # Returns
    ///
    /// Mapping of hashed keys of updated accounts to their respective updated hashed slots.
    fn insert_storage_for_hashing(
        &self,
        storages: impl IntoIterator<Item = (Address, impl IntoIterator<Item = StorageEntry>)>,
    ) -> ProviderResult<B256Map<BTreeSet<B256>>>;
}

crate::macros::impl_provider_refs!(T: HashingWriter, shared_bounds = [Sync] {
    fn unwind_account_hashing<'a>(
        &self,
        changesets: impl Iterator<Item = &'a (BlockNumber, AccountBeforeTx<Self::AccountExtension>)>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account<Self::AccountExtension>>>> {
        T::unwind_account_hashing(&**self, changesets)
    }

    fn unwind_account_hashing_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account<Self::AccountExtension>>>> {
        T::unwind_account_hashing_range(&**self, range)
    }

    fn insert_account_for_hashing(
        &self,
        accounts: impl IntoIterator<Item = (Address, Option<Account<Self::AccountExtension>>)>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account<Self::AccountExtension>>>> {
        T::insert_account_for_hashing(&**self, accounts)
    }

    fn unwind_storage_hashing(
        &self,
        changesets: impl Iterator<Item = (BlockNumberAddress, StorageEntry)>,
    ) -> ProviderResult<B256Map<BTreeSet<B256>>> {
        T::unwind_storage_hashing(&**self, changesets)
    }

    fn unwind_storage_hashing_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<B256Map<BTreeSet<B256>>> {
        T::unwind_storage_hashing_range(&**self, range)
    }

    fn insert_storage_for_hashing(
        &self,
        storages: impl IntoIterator<Item = (Address, impl IntoIterator<Item = StorageEntry>)>,
    ) -> ProviderResult<B256Map<BTreeSet<B256>>> {
        T::insert_storage_for_hashing(&**self, storages)
    }
});
